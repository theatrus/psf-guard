//! Program-bound altitude/meridian screening through the existing goal selector.
//! This cannot widen an allocation and is not yet the production dispatch path.
//! Darkness, conditions, durable reservation and native safety remain required.

use crate::program::{BoundProgram, MAS_PER_DEGREE};
use crate::visibility::{
    altitude_windows, meridian_windows, AltitudeLimits, EarthOrientation, Horizon, IcrsPosition,
    Site, VisibilityError,
};
use crate::windows::{Interval, MeridianExclusion, MAX_WINDOWS};
use crate::{Decision, Request, State, CONTRACT_VERSION};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
mod preparation;
mod priority;
pub use preparation::GeometryPreparation;
pub use priority::PriorityError;

pub const CONSTRAINTS_VERSION: u32 = 1;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GoalLimits {
    pub goal_id: String,
    pub minimum_altitude_degrees: f64,
    pub maximum_altitude_degrees: f64,
    pub horizon_offset_degrees: f64,
}

/// One effective rig snapshot. Revision is not a substitute for comparing its
/// content: reusing an ID cannot hide a changed site, horizon, policy, or EOP.
/// Local file paths and native NINA objects do not belong in this contract.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Constraints {
    pub schema_version: u32,
    pub rig: RigConstraints,
    pub goals: Vec<GoalLimits>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RigConstraints {
    pub rig_id: String,
    pub configuration_id: String,
    pub revision: u64,
    pub site: Site,
    pub orientation: EarthOrientation,
    pub horizon: Horizon,
    pub minimum_altitude_degrees: f64,
    pub maximum_altitude_degrees: f64,
    pub meridian_exclusion: MeridianExclusion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidConstraints,
    ConstraintsChanged,
    WrongScope,
    TooManyWindows,
    Program(crate::program::Error),
    Planning(crate::Error),
    Geometry(VisibilityError),
    Preparation(crate::preparation::Error),
    InvalidCheckpoint,
    InvalidPreferences,
}

/// Own both source intent and computed geometry. This object is intentionally
/// not deserializable: serialized window lists cannot claim computed evidence.
#[derive(Debug)]
pub struct BoundGeometry {
    source: BoundProgram,
    constraints: Constraints,
    windows: BTreeMap<String, Vec<Interval>>,
    non_lunar_windows: BTreeMap<String, Vec<Interval>>,
    priorities: BTreeMap<String, u32>,
}

impl BoundGeometry {
    pub fn constraints(&self) -> &Constraints {
        &self.constraints
    }

    pub fn new(
        source: BoundProgram,
        mut constraints: Constraints,
        state: &State,
    ) -> Result<Self, Error> {
        let assignment = &source.snapshot().assignment;
        if constraints.schema_version != CONSTRAINTS_VERSION
            || constraints.rig.revision == 0
            || constraints.goals.len() != assignment.goals.len()
        {
            return Err(Error::InvalidConstraints);
        }
        if constraints.rig.rig_id != assignment.rig_id
            || constraints.rig.configuration_id != assignment.configuration_id
            || state.rig_id != constraints.rig.rig_id
            || state.configuration_id != constraints.rig.configuration_id
            || state.meridian_exclusion != constraints.rig.meridian_exclusion
        {
            return Err(Error::WrongScope);
        }
        constraints.goals.sort_by(|a, b| a.goal_id.cmp(&b.goal_id));
        if constraints
            .goals
            .windows(2)
            .any(|p| p[0].goal_id == p[1].goal_id)
            || constraints
                .goals
                .iter()
                .any(|g| source.resolve(&g.goal_id).is_err())
        {
            return Err(Error::InvalidConstraints);
        }
        // Retain the legacy transit contract too. Its input may further restrict
        // computed geometry, but cannot relax the independent rig calculation.
        // Cache allocation geometry, not a transient session deadline. Each
        // evaluation clips these windows to its current completion deadline.
        let mut geometry_state = state.clone();
        geometry_state.completion_deadline_ms = None;
        let permitted = crate::validate(&Request {
            contract_version: CONTRACT_VERSION,
            assignment: assignment.clone(),
            state: geometry_state,
        })
        .map_err(Error::Planning)?;
        let span = Interval {
            start_ms: assignment.valid_from_ms,
            end_ms: assignment.expires_at_ms,
        };
        let mut windows = BTreeMap::new();
        let mut non_lunar_windows = BTreeMap::new();
        let mut moon_track = None;
        let mut cache: Vec<(String, AltitudeLimits, Vec<Interval>)> = vec![];
        for (goal, allocated) in assignment.goals.iter().zip(permitted) {
            let resolved = source.resolve(&goal.id).map_err(Error::Program)?;
            let preferences = &constraints.goals[constraints
                .goals
                .binary_search_by(|g| g.goal_id.cmp(&goal.id))
                .map_err(|_| Error::InvalidConstraints)?];
            let limits = AltitudeLimits {
                rig_minimum_degrees: constraints.rig.minimum_altitude_degrees,
                rig_maximum_degrees: constraints.rig.maximum_altitude_degrees,
                project_minimum_degrees: preferences.minimum_altitude_degrees,
                project_maximum_degrees: preferences.maximum_altitude_degrees,
                horizon_offset_degrees: preferences.horizon_offset_degrees,
            };
            let computed = if let Some((_, _, computed)) = cache
                .iter()
                .find(|(id, previous, _)| id == &resolved.target.id && *previous == limits)
            {
                computed
            } else {
                let position = IcrsPosition {
                    ra_degrees: f64::from(resolved.target.icrs_ra_mas) / f64::from(MAS_PER_DEGREE),
                    dec_degrees: f64::from(resolved.target.icrs_dec_mas)
                        / f64::from(MAS_PER_DEGREE),
                };
                let altitude = altitude_windows(
                    position,
                    constraints.rig.site,
                    constraints.rig.orientation,
                    &constraints.rig.horizon,
                    limits,
                    span,
                )
                .map_err(Error::Geometry)?;
                let meridian = meridian_windows(
                    position,
                    constraints.rig.site,
                    constraints.rig.orientation,
                    span,
                    constraints.rig.meridian_exclusion,
                )
                .map_err(Error::Geometry)?;
                let computed = intersect(&altitude.windows, &meridian.windows)?;
                cache.push((resolved.target.id.clone(), limits, computed));
                &cache.last().unwrap().2
            };
            let mut eligible = intersect(&allocated, computed)?;
            non_lunar_windows.insert(goal.id.clone(), eligible.clone());
            if let Some(policy) = &resolved.recipe.moon
                && policy.enabled
            {
                if moon_track.is_none() {
                    moon_track = Some(
                        crate::moon::MoonTrack::new(
                            constraints.rig.site,
                            constraints.rig.orientation,
                            span,
                        )
                        .map_err(Error::Geometry)?,
                    );
                }
                let lunar = moon_track
                    .as_ref()
                    .unwrap()
                    .windows(
                        policy,
                        IcrsPosition {
                            ra_degrees: f64::from(resolved.target.icrs_ra_mas)
                                / f64::from(MAS_PER_DEGREE),
                            dec_degrees: f64::from(resolved.target.icrs_dec_mas)
                                / f64::from(MAS_PER_DEGREE),
                        },
                    )
                    .map_err(Error::Geometry)?;
                eligible = intersect(&eligible, &lunar)?;
            }
            windows.insert(goal.id.clone(), eligible);
        }
        // Preserve objective priority first, then prefer Moon-sensitive work
        // among eligible equal-priority recipes. Compact ranks avoid overflow.
        let mut keys = BTreeMap::new();
        for goal in &assignment.goals {
            let recipe = source.resolve(&goal.id).map_err(Error::Program)?.recipe;
            let aversion = recipe
                .moon
                .as_ref()
                .map_or(Ok(0.0), |p| p.aversion())
                .map_err(|_| Error::InvalidConstraints)?;
            keys.insert(goal.id.clone(), (goal.priority, aversion.to_bits()));
        }
        let ordered: std::collections::BTreeSet<_> = keys.values().copied().collect();
        let ranks: BTreeMap<_, _> = ordered
            .into_iter()
            .enumerate()
            .map(|(rank, key)| (key, rank as u32))
            .collect();
        let mut priorities: BTreeMap<String, u32> = keys
            .into_iter()
            .map(|(id, key)| (id, ranks[&key]))
            .collect();
        if !source
            .snapshot()
            .recipes
            .iter()
            .any(|r| r.moon.as_ref().is_some_and(|p| p.enabled))
        {
            priorities = assignment
                .goals
                .iter()
                .map(|g| (g.id.clone(), g.priority))
                .collect();
        }
        Ok(Self {
            source,
            constraints,
            windows,
            non_lunar_windows,
            priorities,
        })
    }

    /// Diagnostic evidence only, not an acquisition permit or a replacement
    /// assignment. Source intent and its bounds remain owned by this object.
    pub fn windows(&self, goal_id: &str) -> Option<&[Interval]> {
        self.windows.get(goal_id).map(Vec::as_slice)
    }

    /// The caller supplies current conditions and a freshly refreshed constraint
    /// snapshot. Refreshing native files/profile state is the host's duty.
    /// Only progress counters may differ from the original assigned program.
    pub fn evaluate(&self, request: &Request, current: &Constraints) -> Result<Decision, Error> {
        self.evaluate_with_active(request, current, None)
    }

    pub fn evaluate_with_active(
        &self,
        request: &Request,
        current: &Constraints,
        active: Option<&crate::priority::ActiveGoal>,
    ) -> Result<Decision, Error> {
        match &self.source.snapshot().observing_preferences {
            Some(preferences) => self
                .preview_priority(request, current, &preferences.for_goals(), active)
                .map(|r| r.decision)
                .map_err(|e| match e {
                    PriorityError::Geometry(e) => e,
                    PriorityError::Preferences(_) => Error::InvalidPreferences,
                }),
            None => self.evaluate_legacy(request, current),
        }
    }

    fn evaluate_legacy(&self, request: &Request, current: &Constraints) -> Result<Decision, Error> {
        self.source
            .validate_projection(&request.assignment)
            .map_err(Error::Program)?;
        let original = crate::evaluate(request).map_err(Error::Planning)?;
        // Geometry changes must not prevent a safety stop or interrupt an
        // indivisible native operation. Revalidate before any new work or wait.
        if !matches!(original, Decision::Acquire { .. } | Decision::Wait { .. }) {
            return Ok(original);
        }
        self.check_current(request, current)?;
        let narrowed = self.narrow(request)?;
        let result = crate::evaluate(&narrowed).map_err(Error::Planning)?;
        if matches!(&result, Decision::CheckIn { reason } if reason == "no_authorized_feasible_work")
        {
            let mut without_moon = narrowed;
            for goal in &mut without_moon.assignment.goals {
                goal.eligible_windows = self.non_lunar_windows[&goal.id].clone();
            }
            match crate::evaluate(&without_moon).map_err(Error::Planning)? {
                Decision::Acquire { .. } => {
                    return Ok(Decision::Wait {
                        reason: "moon_avoidance".into(),
                    });
                }
                // Moon blocking cannot turn the approaching night boundary
                // into an error once even the non-lunar work no longer fits.
                Decision::Wait { reason } if reason == "observing_night_window_too_short" => {
                    return Ok(Decision::Wait { reason })
                }
                _ => {}
            }
        }
        Ok(result)
    }

    fn check_current(&self, request: &Request, current: &Constraints) -> Result<(), Error> {
        if request.state.meridian_exclusion != self.constraints.rig.meridian_exclusion {
            return Err(Error::ConstraintsChanged);
        }
        if current.goals.len() != self.constraints.goals.len() {
            return Err(Error::ConstraintsChanged);
        }
        let mut goals: Vec<_> = current.goals.iter().collect();
        goals.sort_by(|a, b| a.goal_id.cmp(&b.goal_id));
        if current.schema_version != self.constraints.schema_version
            || current.rig != self.constraints.rig
            || goals
                .iter()
                .zip(&self.constraints.goals)
                .any(|(current, cached)| *current != cached)
        {
            return Err(Error::ConstraintsChanged);
        }
        Ok(())
    }

    fn narrow(&self, request: &Request) -> Result<Request, Error> {
        self.source
            .validate_projection(&request.assignment)
            .map_err(Error::Program)?;
        crate::validate(request).map_err(Error::Planning)?;
        let mut narrowed = request.clone();
        for goal in &mut narrowed.assignment.goals {
            goal.eligible_windows = self.windows[&goal.id].clone();
            goal.priority = self.priorities[&goal.id];
        }
        Ok(narrowed)
    }
}

// Both inputs are already normalized results. Intersection cannot
// create new coverage; cap fragmentation rather than bridging gaps to fit.
fn intersect(left: &[Interval], right: &[Interval]) -> Result<Vec<Interval>, Error> {
    let mut result = vec![];
    let (mut l, mut r) = (0, 0);
    while l < left.len() && r < right.len() {
        let start_ms = left[l].start_ms.max(right[r].start_ms);
        let end_ms = left[l].end_ms.min(right[r].end_ms);
        if start_ms < end_ms {
            if result.len() >= MAX_WINDOWS {
                return Err(Error::TooManyWindows);
            }
            result.push(Interval { start_ms, end_ms });
        }
        if left[l].end_ms <= right[r].end_ms {
            l += 1;
        } else {
            r += 1;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersection_cannot_bridge_gaps_or_silently_truncate() {
        let left: Vec<_> = (0..128)
            .map(|i| Interval {
                start_ms: i * 10,
                end_ms: i * 10 + 9,
            })
            .collect();
        let right: Vec<_> = (0..128)
            .map(|i| Interval {
                start_ms: i * 10 + 5,
                end_ms: i * 10 + 14,
            })
            .collect();
        assert_eq!(intersect(&left, &right), Err(Error::TooManyWindows));
        for l in 0..4 {
            for r in 0..4 {
                let result = intersect(&left[..l], &right[..r]).unwrap();
                for time in 0..50 {
                    let contains = |windows: &[Interval]| {
                        windows
                            .iter()
                            .any(|w| w.start_ms <= time && time < w.end_ms)
                    };
                    assert_eq!(
                        contains(&result),
                        contains(&left[..l]) && contains(&right[..r])
                    );
                }
            }
        }
    }
}
