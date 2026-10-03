use super::*;

/// Bound to a goal, not a name. Geometry producers must supply current evidence;
/// missing optional evidence is neutral, explicitly reported, never invented.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub goal_id: String,
    pub target_id: String,
    pub policy: ResolvedPolicy,
    pub altitude: Option<u16>,
    pub moon_opportunity: Option<u16>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActiveGoal {
    pub goal_id: String,
    pub selected_at_ms: u64,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Contribution {
    pub value: u16,
    pub weight: u16,
    /// Exact numerator; divide the total by the sum of weights once.
    pub weighted_value: u64,
    pub missing: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Score {
    pub goal_id: String,
    pub total: u16,
    pub contributions: BTreeMap<Factor, Contribution>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Ranking {
    pub decision: Decision,
    /// Eligible candidates sorted by score then stable ID, not input order.
    pub candidates: Vec<Score>,
    pub retained_active: bool,
}

/// Preview only: eligibility, pending credit and attempt budgets come from the
/// existing selector. A preference never admits new work or overrides a stop.
/// Use BoundGeometry::preview_priority for computed horizon/Moon screening.
pub fn preview(
    request: &Request,
    candidates: &[Candidate],
    active: Option<&ActiveGoal>,
) -> Result<Ranking, Error> {
    let original = crate::evaluate(request).map_err(Error::Planning)?;
    if !matches!(original, Decision::Acquire { .. }) {
        return Ok(gated(original));
    }
    if candidates.len() != request.assignment.goals.len() {
        return Err(Error::InvalidCandidates);
    }
    let ids: BTreeSet<_> = candidates.iter().map(|c| c.goal_id.as_str()).collect();
    if ids.len() != candidates.len()
        || request
            .assignment
            .goals
            .iter()
            .any(|g| !ids.contains(g.id.as_str()))
        || candidates.iter().any(|c| {
            !crate::valid_id(&c.target_id)
                || c.altitude.is_some_and(|v| v > SCALE)
                || c.moon_opportunity.is_some_and(|v| v > SCALE)
        })
    {
        return Err(Error::InvalidCandidates);
    }
    for candidate in candidates {
        candidate.policy.policy().validate()?;
    }
    if active.is_some_and(|a| {
        !ids.contains(a.goal_id.as_str())
            || a.selected_at_ms > request.state.now_ms
            || a.selected_at_ms < request.assignment.valid_from_ms
    }) {
        return Err(Error::InvalidActiveGoal);
    }
    let active_candidate = active.and_then(|a| candidates.iter().find(|c| c.goal_id == a.goal_id));
    let windows = crate::validate(request).map_err(Error::Planning)?;
    let mut scores = vec![];
    for (goal, windows) in request.assignment.goals.iter().zip(windows) {
        if goal.attempts_remaining == 0
            || u64::from(goal.accepted) + u64::from(goal.pending) >= u64::from(goal.requested)
        {
            continue;
        }
        let cost = goal.exposure_ms + goal.overhead_ms; // Already overflow-validated.
        let Some(window) = windows.iter().find(|w| {
            request.state.now_ms >= w.start_ms
                && request
                    .state
                    .now_ms
                    .checked_add(cost)
                    .is_some_and(|end| end <= w.end_ms)
        }) else {
            continue;
        };
        let candidate = candidates.iter().find(|c| c.goal_id == goal.id).unwrap();
        let policy = candidate.policy.policy();
        let values = [
            (Factor::Importance, Some(policy.importance * 100)),
            (
                Factor::WindowUrgency,
                Some(ratio(cost, window.end_ms - request.state.now_ms)),
            ),
            (Factor::Altitude, candidate.altitude),
            (Factor::MoonOpportunity, candidate.moon_opportunity),
            // Pending reserves work but is not quality-accepted completion.
            (
                Factor::Completion,
                Some(ratio(u64::from(goal.accepted), u64::from(goal.requested))),
            ),
            (Factor::Efficiency, Some(ratio(goal.exposure_ms, cost))),
            (
                Factor::Continuity,
                Some(
                    if active_candidate.is_some_and(|a| a.target_id == candidate.target_id) {
                        SCALE
                    } else {
                        0
                    },
                ),
            ),
        ];
        let contributions: BTreeMap<_, _> = values
            .into_iter()
            .map(|(factor, value)| {
                let weight = policy.weights[&factor];
                let score = value.unwrap_or(SCALE / 2);
                (
                    factor,
                    Contribution {
                        value: score,
                        weight,
                        weighted_value: u64::from(weight) * u64::from(score),
                        missing: value.is_none(),
                    },
                )
            })
            .collect();
        let numerator: u64 = contributions.values().map(|v| v.weighted_value).sum();
        let denominator: u64 = contributions.values().map(|v| u64::from(v.weight)).sum();
        scores.push(Score {
            goal_id: goal.id.clone(),
            total: (numerator / denominator) as u16,
            contributions,
        });
    }
    scores.sort_by(|a, b| {
        b.total
            .cmp(&a.total)
            .then_with(|| a.goal_id.cmp(&b.goal_id))
    });
    let winner = scores.first().ok_or(Error::InvalidCandidates)?;
    let mut selected = winner;
    let mut retained = false;
    let mut reason = "observing_preference_score";
    if let Some((active, candidate)) = active.zip(active_candidate)
        && let Some(current) = scores.iter().find(|s| s.goal_id == active.goal_id)
        && current.goal_id != winner.goal_id
    {
        let policy = candidate.policy.policy();
        if request.state.now_ms - active.selected_at_ms < policy.minimum_dwell_ms
            || winner.total - current.total <= policy.switch_margin
        {
            selected = current;
            retained = true;
            reason = if request.state.now_ms - active.selected_at_ms < policy.minimum_dwell_ms {
                "observing_preference_minimum_dwell"
            } else {
                "observing_preference_switch_margin"
            };
        }
    }
    let decision = Decision::Acquire {
        goal_id: selected.goal_id.clone(),
        reason: reason.into(),
    };
    Ok(Ranking {
        decision,
        candidates: scores,
        retained_active: retained,
    })
}

fn ratio(numerator: u64, denominator: u64) -> u16 {
    ((u128::from(numerator) * u128::from(SCALE) / u128::from(denominator)).min(u128::from(SCALE)))
        as u16
}
