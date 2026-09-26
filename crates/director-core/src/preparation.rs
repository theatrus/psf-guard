//! Native-sequence exposure preparation policy. No device APIs, clocks, or I/O.
//!
//! This reducer is not a dispatch permit or a durable journal. A host must bind
//! it to its session, persist observations, run operations through NINA's native
//! sequence lifecycle, and reserve/revalidate capture separately. Losing this
//! state does not authorize replay of an operation with an unknown outcome.

use crate::{evaluate, valid_id, Assignment, Decision, Request};
use serde::{Deserialize, Serialize};
mod checkpoint;

/// Resolved recipe and local equipment context, immutable for one preparation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub goal_id: String,
    pub target_id: String,
    pub recipe_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub previous_target_id: Option<String>,
    pub filter_id: String,
    pub readout_mode: i16,
    pub mount_parked: bool,
    pub rotator_connected: bool,
    pub enable_slew_center: bool,
    pub dither_every: u32,
    /// None inherits the target cadence; zero explicitly disables dithering.
    #[serde(deserialize_with = "Option::deserialize")]
    pub dither_override: Option<u32>,
    /// Confirmed exposures in this filter since the last successful dither.
    /// Different recipes using the same filter share this counter.
    pub filter_exposures_since_dither: u32,
}

/// Blocking estimates, not deadlines. Hook estimates include their nested work.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Estimates {
    pub unpark_ms: u64,
    pub center_ms: u64,
    pub before_target_ms: u64,
    pub dither_ms: u64,
    pub filter_ms: u64,
    pub readout_ms: u64,
    pub capture_overhead_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Unpark,
    Center { rotate: bool },
    BeforeTarget,
    Dither,
    SwitchFilter { filter_id: String },
    SetReadoutMode { mode: i16 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub preparation_id: String,
    pub ordinal: u32,
    pub goal_id: String,
    pub target_id: String,
    pub recipe_id: String,
    pub operation: Operation,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Next {
    /// Issued once. Calling next again cannot dispatch it a second time.
    Run(Command),
    InFlight {
        ordinal: u32,
    },
    /// A fresh recommendation only; durable reservation and dispatch checks remain.
    ReadyToReserve {
        goal_id: String,
    },
    Decision(Decision),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    Succeeded,
    Failed { reason: String },
    Uncertain { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Completion {
    pub preparation_id: String,
    pub ordinal: u32,
    pub ended_at_ms: u64,
    /// Observed monotonic elapsed time, not a wall-clock subtraction.
    pub elapsed_ms: u64,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub command: Command,
    pub issued_at_ms: u64,
    pub completion: Completion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidContext,
    EstimateOverflow,
    NotSelected,
    InvalidCompletion,
    ConflictingCompletion,
    ClockRegression,
    InvalidCheckpoint,
    Core(crate::Error),
}

#[derive(Debug)]
struct Step {
    operation: Operation,
    estimate_ms: u64,
}

/// One exposure's preparation, driven by reported native operation outcomes.
/// Keep this state in the shared runtime, never reconstruct it in the C# adapter.
#[derive(Debug)]
pub struct Preparation {
    id: String,
    initial: Request,
    estimates: Estimates,
    assignment: Assignment,
    context: Context,
    steps: Vec<Step>,
    capture_overhead_ms: u64,
    cursor: usize,
    pending: Option<(Command, u64)>,
    observations: Vec<Observation>,
    halted: Option<Decision>,
    last_time_ms: u64,
}

impl Preparation {
    pub fn new(
        id: String,
        request: &Request,
        context: Context,
        estimates: Estimates,
    ) -> Result<Self, Error> {
        if [
            &id,
            &context.goal_id,
            &context.target_id,
            &context.recipe_id,
            &context.filter_id,
        ]
        .iter()
        .any(|id| !valid_id(id))
            || context
                .previous_target_id
                .as_ref()
                .is_some_and(|id| !valid_id(id))
            || context.readout_mode < 0
        {
            return Err(Error::InvalidContext);
        }
        // Check the original snapshot before replacing its preparation estimate.
        crate::validate(request).map_err(Error::Core)?;
        let mut steps = Vec::new();
        let mut add = |operation, estimate_ms| {
            steps.push(Step {
                operation,
                estimate_ms,
            })
        };
        let new_target = context.previous_target_id.as_ref() != Some(&context.target_id);
        if context.mount_parked {
            add(Operation::Unpark, estimates.unpark_ms);
        }
        if new_target {
            if context.enable_slew_center {
                add(
                    Operation::Center {
                        rotate: context.rotator_connected,
                    },
                    estimates.center_ms,
                );
            }
            add(Operation::BeforeTarget, estimates.before_target_ms);
        }
        let cadence = context.dither_override.unwrap_or(context.dither_every);
        // TS resets the old target's dither history on a target change.
        if !new_target && cadence > 0 && context.filter_exposures_since_dither >= cadence {
            add(Operation::Dither, estimates.dither_ms);
        }
        add(
            Operation::SwitchFilter {
                filter_id: context.filter_id.clone(),
            },
            estimates.filter_ms,
        );
        add(
            Operation::SetReadoutMode {
                mode: context.readout_mode,
            },
            estimates.readout_ms,
        );
        let preparation = Self {
            id,
            initial: request.clone(),
            estimates,
            assignment: request.assignment.clone(),
            context,
            steps,
            capture_overhead_ms: estimates.capture_overhead_ms,
            cursor: 0,
            pending: None,
            observations: Vec::new(),
            halted: None,
            last_time_ms: request.state.now_ms,
        };
        if !matches!(preparation.evaluate_remaining(request)?, Decision::Acquire { goal_id, .. }
            if goal_id == preparation.context.goal_id)
        {
            return Err(Error::NotSelected);
        }
        Ok(preparation)
    }

    fn evaluate_remaining(&self, request: &Request) -> Result<Decision, Error> {
        let overhead =
            self.steps[self.cursor..]
                .iter()
                .try_fold(self.capture_overhead_ms, |sum, step| {
                    sum.checked_add(step.estimate_ms)
                        .ok_or(Error::EstimateOverflow)
                })?;
        let mut request = request.clone();
        let goal = request
            .assignment
            .goals
            .iter_mut()
            .find(|goal| goal.id == self.context.goal_id)
            .ok_or(Error::InvalidContext)?;
        goal.overhead_ms = overhead;
        evaluate(&request).map_err(Error::Core)
    }

    /// Reevaluate at every native operation boundary. A target switch, stale
    /// context, or failure ends this preparation; the host cannot skip to capture.
    pub fn next(&mut self, request: &Request) -> Result<Next, Error> {
        self.next_with_constraint_change(request, false)
    }

    /// Recheck the exact issued command after inherited native hooks, before
    /// starting its operation. This issues nothing and cannot authorize replay
    /// after recovery: the host still needs its original one-shot Run command.
    /// Include the pending step's full estimate because it has not started yet.
    pub fn check_pending_dispatch(
        &mut self,
        request: &Request,
        command: &Command,
    ) -> Result<Decision, Error> {
        self.check_pending_dispatch_with_constraint_change(request, command, false)
    }

    pub(crate) fn check_pending_dispatch_with_constraint_change(
        &mut self,
        request: &Request,
        command: &Command,
        constraints_changed: bool,
    ) -> Result<Decision, Error> {
        if self.pending() != Some(command) {
            return Err(Error::InvalidContext);
        }
        // Keep ordinary pending polls unchanged: they wait for a receipt and
        // must not reinterpret a running operation as new work.
        if let Next::Decision(decision) =
            self.next_with_constraint_change(request, constraints_changed)?
        {
            return Ok(decision);
        }
        if let Some(decision) = &self.halted {
            return Ok(decision.clone());
        }
        let decision = match self.evaluate_remaining(request)? {
            Decision::Acquire { goal_id, .. } if goal_id != self.context.goal_id => {
                Decision::CheckIn {
                    reason: "preparation_goal_changed".into(),
                }
            }
            other => other,
        };
        if !matches!(
            decision,
            Decision::Acquire { .. } | Decision::Continue { .. }
        ) {
            self.halted = Some(decision.clone());
        }
        Ok(decision)
    }

    pub(crate) fn next_with_constraint_change(
        &mut self,
        request: &Request,
        constraints_changed: bool,
    ) -> Result<Next, Error> {
        crate::validate(request).map_err(Error::Core)?;
        if request.state.now_ms < self.last_time_ms {
            return Err(Error::ClockRegression);
        }
        if constraints_changed && self.halted.is_none() {
            self.halted = Some(Decision::CheckIn {
                reason: "observing_constraints_changed".into(),
            });
        }
        let next = self.advance(request)?;
        self.last_time_ms = request.state.now_ms;
        Ok(next)
    }

    fn advance(&mut self, request: &Request) -> Result<Next, Error> {
        // Check safety before sticky check-in decisions or looking up the old
        // goal: a replacement assignment may no longer contain that goal.
        let decision = evaluate(request).map_err(Error::Core)?;
        if matches!(decision, Decision::Stop { .. }) {
            self.halted = Some(decision.clone());
            return Ok(Next::Decision(decision));
        }
        if self.halted.is_none() && request.assignment != self.assignment {
            self.halted = Some(Decision::CheckIn {
                reason: "preparation_assignment_changed".into(),
            });
        }
        if self.halted.is_none()
            && (request.state.rig_id != self.assignment.rig_id
                || request.state.configuration_id != self.assignment.configuration_id)
        {
            self.halted = Some(Decision::CheckIn {
                reason: "configuration_mismatch".into(),
            });
        }
        if let Some(decision @ Decision::Stop { .. }) = &self.halted {
            return Ok(Next::Decision(decision.clone()));
        }
        if let Some((command, _)) = &self.pending {
            // Reprioritization never interrupts an indivisible native action.
            // Latch a changed assignment, then wait for its correlated receipt.
            return Ok(Next::InFlight {
                ordinal: command.ordinal,
            });
        }
        if let Some(decision) = &self.halted {
            return Ok(Next::Decision(decision.clone()));
        }
        let decision = self.evaluate_remaining(request)?;
        if matches!(decision, Decision::Continue { .. }) {
            return Ok(Next::Decision(decision));
        }
        if !matches!(&decision, Decision::Acquire { goal_id, .. } if goal_id == &self.context.goal_id)
        {
            // This preparation cannot prepare a different recipe or retain an
            // Acquire recommendation that may be stale on the next call.
            let decision = match decision {
                Decision::Acquire { .. } => Decision::CheckIn {
                    reason: "preparation_goal_changed".into(),
                },
                other => other,
            };
            self.halted = Some(decision.clone());
            return Ok(Next::Decision(decision));
        }
        let Some(step) = self.steps.get(self.cursor) else {
            return Ok(Next::ReadyToReserve {
                goal_id: self.context.goal_id.clone(),
            });
        };
        let command = Command {
            preparation_id: self.id.clone(),
            ordinal: self.cursor as u32 + 1,
            goal_id: self.context.goal_id.clone(),
            target_id: self.context.target_id.clone(),
            recipe_id: self.context.recipe_id.clone(),
            operation: step.operation.clone(),
        };
        self.pending = Some((command.clone(), request.state.now_ms));
        Ok(Next::Run(command))
    }

    pub fn complete(&mut self, completion: Completion) -> Result<(), Error> {
        if completion.preparation_id != self.id || completion.ordinal == 0 {
            return Err(Error::InvalidCompletion);
        }
        if let Some(observed) = self
            .observations
            .iter()
            .find(|item| item.command.ordinal == completion.ordinal)
        {
            return if observed.completion == completion {
                Ok(())
            } else {
                Err(Error::ConflictingCompletion)
            };
        }
        let Some((command, issued_at_ms)) = &self.pending else {
            return Err(Error::InvalidCompletion);
        };
        if completion.ordinal != command.ordinal
            || completion.ended_at_ms < *issued_at_ms
            || matches!(&completion.outcome, Outcome::Failed { reason } | Outcome::Uncertain { reason } if !valid_id(reason))
        {
            return Err(Error::InvalidCompletion);
        }
        let halted = match &completion.outcome {
            Outcome::Succeeded => None,
            Outcome::Failed { .. } => Some(Decision::CheckIn {
                reason: "preparation_failed".into(),
            }),
            Outcome::Uncertain { .. } => Some(Decision::CheckIn {
                reason: "preparation_uncertain".into(),
            }),
        };
        // A receipt can arrive after a poll whose timestamp is newer than the
        // operation's end. Accept it without moving our snapshot clock backwards.
        self.last_time_ms = self.last_time_ms.max(completion.ended_at_ms);
        self.observations.push(Observation {
            command: command.clone(),
            issued_at_ms: *issued_at_ms,
            completion,
        });
        self.pending = None;
        self.cursor += 1;
        if self.halted.is_none() {
            self.halted = halted;
        }
        Ok(())
    }

    /// Outer operation timings only. Nested native-trigger timings must be
    /// recorded as children, not added again to these blocking durations.
    pub fn observations(&self) -> &[Observation] {
        &self.observations
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn context(&self) -> &Context {
        &self.context
    }

    pub fn estimates(&self) -> Estimates {
        self.estimates
    }

    pub fn pending(&self) -> Option<&Command> {
        self.pending.as_ref().map(|(command, _)| command)
    }

    pub fn halted(&self) -> Option<&Decision> {
        self.halted.as_ref()
    }

    /// Informational only. Call next with fresh state before any reservation.
    pub fn steps_completed(&self) -> bool {
        self.cursor == self.steps.len() && self.pending.is_none() && self.halted.is_none()
    }
}
