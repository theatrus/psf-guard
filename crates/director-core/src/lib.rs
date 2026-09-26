//! Phase-0 goal-selection spike. No I/O, clock access, or hardware control.
//! Selection still uses precomputed eligibility windows. The visibility module
//! supplies shared point geometry, not yet full observing-window construction.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub mod preparation;
pub mod program;
pub mod visibility;
pub mod windows;
use windows::{observing_windows, Interval, MeridianExclusion, TransitCoverage, WindowError};

pub const CONTRACT_VERSION: u32 = 2;
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const MAX_REQUEST_BYTES: usize = 262_144;
const MAX_GOALS: usize = 256;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub contract_version: u32,
    pub assignment: Assignment,
    pub state: State,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub id: String,
    pub revision: u64,
    pub rig_id: String,
    pub configuration_id: String,
    pub valid_from_ms: u64,
    pub expires_at_ms: u64,
    pub goals: Vec<Goal>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    pub id: String,
    pub priority: u32,
    pub requested: u32,
    pub accepted: u32,
    pub pending: u32,
    /// Remaining authorized attempts, not requested minus accepted images.
    pub attempts_remaining: u32,
    pub exposure_ms: u64,
    /// Host estimate of additional blocking work before this exposure completes.
    pub overhead_ms: u64,
    pub eligible_windows: Vec<Interval>,
    pub transits: Option<TransitCoverage>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub rig_id: String,
    pub configuration_id: String,
    pub now_ms: u64,
    pub conditions_valid_until_ms: u64,
    pub safety: Safety,
    pub at_boundary: bool,
    pub operator_stop: bool,
    pub meridian_exclusion: MeridianExclusion,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Safety {
    Safe,
    Unsafe,
    Unknown,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Decision {
    Acquire { goal_id: String, reason: String },
    Continue { reason: String },
    Stop { reason: String },
    CheckIn { reason: String },
    Wait { reason: String },
    Complete { reason: String },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Outcome {
    Ok { decision: Decision },
    Error { code: Error },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Response {
    pub contract_version: u32,
    pub engine_version: String,
    pub assignment_id: Option<String>,
    pub assignment_revision: Option<u64>,
    #[serde(flatten)]
    pub outcome: Outcome,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    RequestTooLarge,
    InvalidJson,
    UnsupportedContract,
    InvalidAssignment,
    InvalidState,
    DuplicateGoal,
    InvalidGoal,
    InvalidWindows,
    InvalidTransits,
    IncompleteTransitCoverage,
    MeridianTimeOverflow,
}

impl From<WindowError> for Error {
    fn from(error: WindowError) -> Self {
        match error {
            WindowError::InvalidWindows => Self::InvalidWindows,
            WindowError::InvalidTransits => Self::InvalidTransits,
            WindowError::IncompleteTransitCoverage => Self::IncompleteTransitCoverage,
            WindowError::MeridianTimeOverflow => Self::MeridianTimeOverflow,
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|c| c.is_ascii_graphic())
}

fn validate(request: &Request) -> Result<Vec<Vec<Interval>>, Error> {
    if request.contract_version != CONTRACT_VERSION {
        return Err(Error::UnsupportedContract);
    }
    let a = &request.assignment;
    if !valid_id(&a.id)
        || !valid_id(&a.rig_id)
        || !valid_id(&a.configuration_id)
        || a.revision == 0
        || a.valid_from_ms >= a.expires_at_ms
        || a.goals.is_empty()
        || a.goals.len() > MAX_GOALS
    {
        return Err(Error::InvalidAssignment);
    }
    if !valid_id(&request.state.rig_id) || !valid_id(&request.state.configuration_id) {
        return Err(Error::InvalidState);
    }
    let mut ids = BTreeSet::new();
    let mut safe_windows = Vec::new();
    for goal in &a.goals {
        if !ids.insert(&goal.id) {
            return Err(Error::DuplicateGoal);
        }
        if !valid_id(&goal.id)
            || goal.requested == 0
            || goal.exposure_ms == 0
            || goal.exposure_ms.checked_add(goal.overhead_ms).is_none()
        {
            return Err(Error::InvalidGoal);
        }
        safe_windows.push(observing_windows(
            &goal.eligible_windows,
            Interval {
                start_ms: a.valid_from_ms,
                end_ms: a.expires_at_ms,
            },
            request.state.meridian_exclusion,
            goal.transits.as_ref(),
        )?);
    }
    Ok(safe_windows)
}

/// Select one authorized next exposure. The caller must durably reserve the
/// attempt and revalidate safety/state at dispatch; this pure function reserves nothing.
pub fn evaluate(request: &Request) -> Result<Decision, Error> {
    let safe_windows = validate(request)?;
    let a = &request.assignment;
    let s = &request.state;
    if s.operator_stop {
        return Ok(Decision::Stop {
            reason: "operator_stop".into(),
        });
    }
    if s.safety != Safety::Safe {
        return Ok(Decision::Stop {
            reason: "safety_not_confirmed".into(),
        });
    }
    // Reprioritization and expiry must not interrupt an indivisible operation.
    // NINA's local safety handling remains independent of this recommendation.
    if !s.at_boundary {
        return Ok(Decision::Continue {
            reason: "await_operation_boundary".into(),
        });
    }
    if s.rig_id != a.rig_id || s.configuration_id != a.configuration_id {
        return Ok(Decision::CheckIn {
            reason: "configuration_mismatch".into(),
        });
    }
    if s.now_ms >= a.expires_at_ms {
        return Ok(Decision::CheckIn {
            reason: "assignment_expired".into(),
        });
    }
    if s.now_ms < a.valid_from_ms {
        return Ok(Decision::Wait {
            reason: "assignment_not_started".into(),
        });
    }
    if s.now_ms >= s.conditions_valid_until_ms {
        return Ok(Decision::CheckIn {
            reason: "conditions_stale".into(),
        });
    }
    if a.goals.iter().all(|g| g.accepted >= g.requested) {
        return Ok(Decision::Complete {
            reason: "accepted_goal_met".into(),
        });
    }

    let remaining: Vec<_> = a
        .goals
        .iter()
        .zip(&safe_windows)
        .filter(|(g, _)| u64::from(g.accepted) + u64::from(g.pending) < u64::from(g.requested))
        .collect();
    if remaining.is_empty() {
        return Ok(Decision::Wait {
            reason: "pending_assessment".into(),
        });
    }

    let fits = |g: &Goal, window: &Interval, start: u64| {
        start
            .checked_add(g.exposure_ms)
            .and_then(|v| v.checked_add(g.overhead_ms))
            .is_some_and(|end| start >= window.start_ms && end <= window.end_ms)
    };
    let selected = remaining
        .iter()
        .copied()
        .filter(|(g, windows)| {
            g.attempts_remaining > 0 && windows.iter().any(|w| fits(g, w, s.now_ms))
        })
        .min_by(|left, right| {
            right
                .0
                .priority
                .cmp(&left.0.priority)
                .then_with(|| left.0.id.cmp(&right.0.id))
        });
    if let Some((goal, _)) = selected {
        return Ok(Decision::Acquire {
            goal_id: goal.id.clone(),
            reason: "highest_priority_feasible_goal".into(),
        });
    }
    if remaining.iter().any(|(g, windows)| {
        g.attempts_remaining > 0
            && windows
                .iter()
                .any(|w| w.start_ms > s.now_ms && fits(g, w, w.start_ms))
    }) {
        return Ok(Decision::Wait {
            reason: "future_window".into(),
        });
    }
    Ok(Decision::CheckIn {
        reason: "no_authorized_feasible_work".into(),
    })
}

/// Bounded JSON contract shared with the native host. Invalid input never yields work.
pub fn evaluate_json(input: &[u8]) -> Response {
    let request = if input.len() > MAX_REQUEST_BYTES {
        Err(Error::RequestTooLarge)
    } else {
        serde_json::from_slice::<Request>(input).map_err(|_| Error::InvalidJson)
    };
    let (assignment_id, assignment_revision, outcome) = match request {
        Ok(request) => {
            let outcome = match evaluate(&request) {
                Ok(decision) => Outcome::Ok { decision },
                Err(code) => Outcome::Error { code },
            };
            (
                Some(request.assignment.id),
                Some(request.assignment.revision),
                outcome,
            )
        }
        Err(code) => (None, None, Outcome::Error { code }),
    };
    Response {
        contract_version: CONTRACT_VERSION,
        engine_version: ENGINE_VERSION.into(),
        assignment_id,
        assignment_revision,
        outcome,
    }
}
