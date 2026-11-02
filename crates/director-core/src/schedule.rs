//! A virtual night for a Director rig: step a clock from dusk to dawn and,
//! at every step, ask [`BoundGeometry::evaluate`] what to do, as the rig's
//! own dispatch does. The schedule is a planning estimate of the order of
//! work and roughly when it happens, not a promise of clock times; the rig
//! still decides on the night.
//!
//! There is no second scheduler here: every choice is the geometry's.

use crate::geometry::{BoundGeometry, Constraints, Error as GeometryError};
use crate::overrides::Narrowing;
use crate::program::{BoundProgram, Program};
use crate::windows::Interval;
use crate::{Decision, Request, Safety, State, CONTRACT_VERSION};
use serde::Serialize;
use std::collections::BTreeMap;

/// Most steps one night may take.
const MAX_STEPS_PER_NIGHT: usize = 20_000;

/// One night to schedule: noon to noon, and the dark part of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScheduleNight {
    pub span: Interval,
    /// `None` when the night never gets dark.
    pub dark: Option<Interval>,
}

/// Estimated time the rig spends on work around exposures, on top of each
/// goal's own overhead.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Overheads {
    /// Slew, centre and rotate on a target change.
    pub target_change_ms: u64,
    pub filter_change_ms: u64,
    pub dither_ms: u64,
}

impl Default for Overheads {
    fn default() -> Self {
        Self {
            target_change_ms: 90_000,
            filter_change_ms: 10_000,
            dither_ms: 15_000,
        }
    }
}

pub struct ScheduleRequest<'a> {
    /// The rig's program as it would pull it. Its assignment's span and the
    /// goals' windows are replaced night by night.
    pub program: &'a Program,
    pub constraints: &'a Constraints,
    pub nights: &'a [ScheduleNight],
    /// Nothing is planned before this instant.
    pub start_ms: u64,
    /// When the rig may plan to image; `None` is always.
    pub available: Option<&'a [Interval]>,
    pub overheads: Overheads,
    /// The share of frames each goal, by id, has had accepted; a goal not
    /// listed counts every frame.
    pub acceptance: &'a BTreeMap<String, f64>,
    /// Each night's overrides, by the night's index; a night past the end
    /// of the slice, or `None`, has none.
    pub narrowings: &'a [Option<Narrowing>],
}

/// One planned exposure.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlannedExposure {
    pub goal_id: String,
    pub target_id: String,
    pub recipe_id: String,
    pub filter_id: String,
    pub exposure_ms: u64,
    /// When the work for it starts: any slew, filter change and dither first.
    pub start_ms: u64,
    pub end_ms: u64,
    pub target_change: bool,
    pub filter_change: bool,
    pub dither: bool,
    /// The geometry's reason, such as `highest_priority_feasible_goal`.
    pub reason: String,
    /// The goal's priority as the geometry ranked it.
    pub priority: u32,
}

/// Why nothing is planned over a stretch of a night.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    /// Before dusk or after dawn.
    Twilight,
    /// Outside the night's available windows.
    Unavailable,
    /// The night's overrides hold the rig.
    Held,
    /// The night's overrides exclude every goal still owed.
    Excluded,
    /// Work fits the sky, but no recipe's Moon rules allow it yet.
    Moon,
    /// No goal's target is inside its limits yet.
    BelowLimits,
    /// Every goal has its frames.
    Complete,
    /// Goals are owed, but none fits before the night, or the clear window,
    /// ends.
    NothingEligible,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlannedGap {
    pub start_ms: u64,
    pub end_ms: u64,
    pub reason: GapReason,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlannedNight {
    pub exposures: Vec<PlannedExposure>,
    pub gaps: Vec<PlannedGap>,
}

/// One goal's frames over the range.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GoalProgress {
    pub goal_id: String,
    pub requested: u32,
    pub accepted_before: u32,
    /// Frames planned on each night of the range.
    pub planned: Vec<u32>,
    /// The night, by index, after which the goal owes nothing.
    pub completes_on: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Schedule {
    pub nights: Vec<PlannedNight>,
    pub progress: Vec<GoalProgress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The program, bound to a night, does not validate.
    Program(crate::program::Error),
    Geometry(GeometryError),
    /// The geometry stopped or asked for an operation boundary, which a
    /// virtual night never reaches.
    Unexpected,
}

fn push_gap(night: &mut PlannedNight, start: u64, end: u64, reason: GapReason) {
    if end <= start {
        return;
    }
    if let Some(last) = night.gaps.last_mut()
        && last.end_ms == start
        && last.reason == reason
    {
        last.end_ms = end;
        return;
    }
    night.gaps.push(PlannedGap {
        start_ms: start,
        end_ms: end,
        reason,
    });
}

/// The available window holding an instant, or the next one's start.
enum Clear {
    Until(u64),
    From(u64),
    Never,
}

fn clear_at(available: Option<&[Interval]>, at: u64) -> Clear {
    let Some(windows) = available else {
        return Clear::Until(u64::MAX);
    };
    let mut next: Option<u64> = None;
    for window in windows {
        if window.start_ms <= at && at < window.end_ms {
            return Clear::Until(window.end_ms);
        }
        if window.start_ms > at && next.is_none_or(|n| window.start_ms < n) {
            next = Some(window.start_ms);
        }
    }
    next.map_or(Clear::Never, Clear::From)
}

/// A wait from `start` to `end`, labelled `reason` except where the
/// night's overrides exclude every goal still owed.
fn push_wait(
    night: &mut PlannedNight,
    narrowing: Option<&Narrowing>,
    goals: &[crate::Goal],
    start: u64,
    end: u64,
    reason: GapReason,
) {
    let Some(narrowing) = narrowing else {
        push_gap(night, start, end, reason);
        return;
    };
    // The stretches where exclusion can change: between the lost windows'
    // starts and ends.
    let mut cuts: Vec<u64> = narrowing
        .lost
        .values()
        .flatten()
        .flat_map(|window| [window.start_ms, window.end_ms])
        .filter(|at| *at > start && *at < end)
        .collect();
    cuts.push(end);
    cuts.sort_unstable();
    cuts.dedup();
    let mut from = start;
    for cut in cuts {
        let label = if excludes_every_owed_goal(narrowing, goals, from) {
            GapReason::Excluded
        } else {
            reason
        };
        push_gap(night, from, cut, label);
        from = cut;
    }
}

/// Whether the night's overrides take `at` from every goal still owed.
fn excludes_every_owed_goal(narrowing: &Narrowing, goals: &[crate::Goal], at: u64) -> bool {
    let mut owed = goals
        .iter()
        .filter(|g| g.accepted < g.requested && g.attempts_remaining > 0)
        .peekable();
    owed.peek().is_some()
        && owed.all(|goal| {
            narrowing.lost.get(&goal.id).is_some_and(|lost| {
                lost.iter()
                    .any(|window| window.start_ms <= at && at < window.end_ms)
            })
        })
}

/// Run the rig's selection over the nights.
pub fn simulate(request: &ScheduleRequest) -> Result<Schedule, Error> {
    let constraints = request.constraints;
    let rig = &constraints.rig;
    let mut goals = request.program.assignment.goals.clone();
    // Each night starts from the program's priorities: a night's own plan
    // order lasts that night only.
    let priorities: BTreeMap<String, u32> =
        goals.iter().map(|g| (g.id.clone(), g.priority)).collect();
    // Accepted frames as the run goes, fractional when frames count at an
    // acceptance rate.
    let mut accepted: BTreeMap<String, f64> = goals
        .iter()
        .map(|g| (g.id.clone(), f64::from(g.accepted)))
        .collect();
    let mut progress: Vec<GoalProgress> = goals
        .iter()
        .map(|g| GoalProgress {
            goal_id: g.id.clone(),
            requested: g.requested,
            accepted_before: g.accepted,
            planned: vec![0; request.nights.len()],
            completes_on: None,
        })
        .collect();
    let mut nights = Vec::with_capacity(request.nights.len());
    for (index, night) in request.nights.iter().enumerate() {
        let mut planned = PlannedNight {
            exposures: vec![],
            gaps: vec![],
        };
        let Some(dark) = night.dark else {
            push_gap(
                &mut planned,
                night.span.start_ms,
                night.span.end_ms,
                GapReason::Twilight,
            );
            nights.push(planned);
            continue;
        };
        let mut program = request.program.clone();
        program.assignment.valid_from_ms = night.span.start_ms;
        program.assignment.expires_at_ms = night.span.end_ms;
        for goal in &mut goals {
            goal.eligible_windows = vec![night.span];
            goal.priority = priorities[&goal.id];
        }
        let narrowing = request.narrowings.get(index).and_then(Option::as_ref);
        if let Some(narrowing) = narrowing {
            narrowing.apply(&mut goals);
            // The night's plan order stands in place of the rig's, as it
            // does in the program the rig pulls.
            if narrowing.priorities.is_some() {
                program.observing_preferences = None;
            }
        }
        let held_from = narrowing
            .and_then(Narrowing::held)
            .map(|held| held.start_ms);
        program.assignment.goals = goals.clone();
        let state = |now_ms: u64, deadline: Option<u64>| State {
            rig_id: rig.rig_id.clone(),
            configuration_id: rig.configuration_id.clone(),
            now_ms,
            conditions_valid_until_ms: night.span.end_ms,
            completion_deadline_ms: deadline,
            safety: Safety::Safe,
            at_boundary: true,
            operator_stop: false,
            meridian_exclusion: rig.meridian_exclusion,
        };
        let bound = BoundProgram::new(program, &state(night.span.start_ms, None))
            .map_err(Error::Program)?;
        let geometry = BoundGeometry::new(
            bound,
            constraints.clone(),
            &state(night.span.start_ms, None),
        )
        .map_err(Error::Geometry)?;
        let mut at = dark.start_ms.max(request.start_ms).max(night.span.start_ms);
        push_gap(
            &mut planned,
            night.span.start_ms,
            at.min(night.span.end_ms),
            GapReason::Twilight,
        );
        let mut last_target: Option<String> = None;
        let mut last_filter: Option<String> = None;
        let mut since_dither: u32 = 0;
        let dither_every = request.program.configuration.dither_every;
        let mut steps = 0;
        while at < dark.end_ms && steps < MAX_STEPS_PER_NIGHT {
            steps += 1;
            if let Some(held) = held_from
                && at >= held
            {
                push_gap(&mut planned, at, dark.end_ms, GapReason::Held);
                at = dark.end_ms;
                break;
            }
            let window_end = match clear_at(request.available, at) {
                Clear::Until(until) => until,
                Clear::From(from) => {
                    let until = from.min(dark.end_ms);
                    push_gap(&mut planned, at, until, GapReason::Unavailable);
                    at = until;
                    last_target = None;
                    continue;
                }
                Clear::Never => {
                    push_gap(&mut planned, at, dark.end_ms, GapReason::Unavailable);
                    break;
                }
            };
            let deadline = dark
                .end_ms
                .min(window_end)
                .min(held_from.unwrap_or(u64::MAX));
            let assignment = geometry.assignment_with_progress(&goals);
            let decision = geometry
                .evaluate(
                    &Request {
                        contract_version: CONTRACT_VERSION,
                        assignment,
                        state: state(at, Some(deadline)),
                    },
                    constraints,
                )
                .map_err(Error::Geometry)?;
            match decision {
                Decision::Acquire { goal_id, reason } => {
                    let resolved = geometry.resolve(&goal_id).map_err(Error::Program)?;
                    let target_change = last_target.as_deref() != Some(resolved.target.id.as_str());
                    let filter_change =
                        last_filter.as_deref() != Some(resolved.recipe.filter_id.as_str());
                    if target_change {
                        since_dither = 0;
                    }
                    let dither = dither_every > 0 && since_dither >= dither_every;
                    let goal = goals
                        .iter_mut()
                        .find(|g| g.id == goal_id)
                        .ok_or(Error::Unexpected)?;
                    let before = u64::from(target_change) * request.overheads.target_change_ms
                        + u64::from(filter_change) * request.overheads.filter_change_ms
                        + u64::from(dither) * request.overheads.dither_ms;
                    let end = at + before + goal.exposure_ms + goal.overhead_ms;
                    planned.exposures.push(PlannedExposure {
                        goal_id: goal_id.clone(),
                        target_id: resolved.target.id.clone(),
                        recipe_id: resolved.recipe.id.clone(),
                        filter_id: resolved.recipe.filter_id.clone(),
                        exposure_ms: goal.exposure_ms,
                        start_ms: at,
                        end_ms: end,
                        target_change,
                        filter_change,
                        dither,
                        reason,
                        priority: geometry.priority(&goal_id).unwrap_or(goal.priority),
                    });
                    since_dither = if dither { 1 } else { since_dither + 1 };
                    last_target = Some(resolved.target.id.clone());
                    last_filter = Some(resolved.recipe.filter_id.clone());
                    let rate = request.acceptance.get(&goal_id).copied().unwrap_or(1.0);
                    let count = accepted.entry(goal_id.clone()).or_default();
                    *count += rate;
                    goal.accepted = (count.floor() as u32).min(goal.requested);
                    goal.attempts_remaining = goal.attempts_remaining.saturating_sub(1);
                    if let Some(entry) = progress.iter_mut().find(|p| p.goal_id == goal_id) {
                        entry.planned[index] += 1;
                    }
                    at = end;
                }
                Decision::Wait { reason } => {
                    let gap = if reason == "moon_avoidance" {
                        GapReason::Moon
                    } else {
                        GapReason::BelowLimits
                    };
                    // The next instant any owed goal's window opens.
                    let next = goals
                        .iter()
                        .filter(|g| g.accepted < g.requested && g.attempts_remaining > 0)
                        .filter_map(|g| geometry.windows(&g.id))
                        .flat_map(|windows| windows.iter())
                        .map(|w| w.start_ms)
                        .filter(|start| *start > at)
                        .min();
                    match next {
                        Some(next)
                            if next < deadline && reason != "observing_night_window_too_short" =>
                        {
                            push_wait(&mut planned, narrowing, &goals, at, next, gap);
                            at = next;
                        }
                        _ => {
                            let reason = if reason == "observing_night_window_too_short" {
                                GapReason::NothingEligible
                            } else {
                                gap
                            };
                            push_wait(&mut planned, narrowing, &goals, at, deadline, reason);
                            at = deadline;
                        }
                    }
                    last_target = None;
                }
                Decision::Complete { reason } => {
                    let gap = if reason == "observing_night_ended" {
                        GapReason::Twilight
                    } else {
                        GapReason::Complete
                    };
                    push_gap(&mut planned, at, dark.end_ms, gap);
                    at = dark.end_ms;
                }
                Decision::CheckIn { .. } => {
                    push_gap(&mut planned, at, deadline, GapReason::NothingEligible);
                    at = deadline;
                    last_target = None;
                }
                Decision::Stop { .. } | Decision::Continue { .. } => return Err(Error::Unexpected),
            }
        }
        push_gap(
            &mut planned,
            at.max(dark.end_ms).min(night.span.end_ms),
            night.span.end_ms,
            GapReason::Twilight,
        );
        for entry in &mut progress {
            if entry.completes_on.is_none()
                && entry.accepted_before < entry.requested
                && goals
                    .iter()
                    .any(|g| g.id == entry.goal_id && g.accepted >= g.requested)
            {
                entry.completes_on = Some(index);
            }
        }
        nights.push(planned);
    }
    Ok(Schedule { nights, progress })
}
