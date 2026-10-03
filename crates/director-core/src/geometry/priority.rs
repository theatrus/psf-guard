//! Diagnostic ranking bound to the same geometry and hard Moon windows.
use super::*;
use crate::priority::{self, ActiveGoal, Candidate, Ranking, ResolvedPolicy, SCALE};

#[derive(Debug, PartialEq, Eq)]
pub enum PriorityError {
    Geometry(Error),
    Preferences(priority::Error),
}

impl BoundGeometry {
    /// Preview user preferences without changing the existing dispatch contract.
    /// Policies must cover every bound goal exactly. No caller-supplied altitude
    /// or Moon score can bypass the computed rig windows.
    pub fn preview_priority(
        &self,
        request: &Request,
        current: &Constraints,
        policies: &BTreeMap<String, ResolvedPolicy>,
        active: Option<&ActiveGoal>,
    ) -> Result<Ranking, PriorityError> {
        let decision = self
            .evaluate_legacy(request, current)
            .map_err(PriorityError::Geometry)?;
        if !matches!(decision, Decision::Acquire { .. }) {
            return Ok(Ranking {
                decision,
                candidates: vec![],
                retained_active: false,
            });
        }
        if policies.len() != request.assignment.goals.len()
            || request
                .assignment
                .goals
                .iter()
                .any(|g| !policies.contains_key(&g.id))
        {
            return Err(PriorityError::Preferences(
                priority::Error::InvalidCandidates,
            ));
        }
        let mut observer = crate::visibility::Observer::at(
            current.rig.site,
            current.rig.orientation,
            request.state.now_ms,
        )
        .map_err(|e| PriorityError::Geometry(Error::Geometry(e)))?;
        let mut candidates = vec![];
        for goal in &request.assignment.goals {
            let resolved = self
                .source
                .resolve(&goal.id)
                .map_err(|e| PriorityError::Geometry(Error::Program(e)))?;
            let observed = observer
                .observe(IcrsPosition {
                    ra_degrees: f64::from(resolved.target.icrs_ra_mas) / f64::from(MAS_PER_DEGREE),
                    dec_degrees: f64::from(resolved.target.icrs_dec_mas)
                        / f64::from(MAS_PER_DEGREE),
                })
                .map_err(|e| PriorityError::Geometry(Error::Geometry(e)))?;
            // A simple geometric quality preference, not pixel-derived quality.
            let altitude = observed.altitude_degrees.to_radians().sin().clamp(0.0, 1.0);
            // Prefer sensitive recipes while their hard lunar windows permit
            // them. This is the recipe's declared sensitivity, not a sky model.
            let moon = resolved
                .recipe
                .moon
                .as_ref()
                .map_or(Ok(0.0), |p| p.aversion())
                .map_err(|_| PriorityError::Geometry(Error::InvalidConstraints))?;
            candidates.push(Candidate {
                goal_id: goal.id.clone(),
                target_id: resolved.target.id.clone(),
                policy: policies[&goal.id].clone(),
                altitude: Some((altitude * f64::from(SCALE)).round() as u16),
                moon_opportunity: Some((moon * f64::from(SCALE)).round() as u16),
            });
        }
        priority::preview(
            &self.narrow(request).map_err(PriorityError::Geometry)?,
            &candidates,
            active,
        )
        .map_err(PriorityError::Preferences)
    }
}
