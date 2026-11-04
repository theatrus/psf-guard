// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A run of the planner over a range of nights, as Target Scheduler's
//! Scheduler Preview runs it: plan, take the exposure or wait, and plan
//! again from where that ended. Each night's work comes back as blocks of
//! exposures and gaps, each with its reason, and the frames planned carry
//! from night to night.

use std::collections::BTreeMap;

use psf_guard_director_core::windows::Interval;
use serde::Serialize;

use crate::planner::{
    DoneReason, NinaProfile, NoNight, Plan, Planner, Sampling, Score, TargetReason,
};
use crate::snapshot::Snapshot;
use crate::twilight::Clock;

/// Most planner calls in one night; a night of one-second exposures stays
/// well inside it.
const MAX_CALLS_PER_NIGHT: usize = 50_000;

/// How faithfully to follow Target Scheduler.
#[derive(Clone, Debug, PartialEq)]
pub enum Mode {
    /// As Target Scheduler's own Scheduler Preview: its coarser sampling,
    /// exposure time only, and every frame accepted when the project
    /// grades. Use it to compare with that preview.
    Preview,
    /// As a live run is expected to go: Target Scheduler's live sampling,
    /// time for slewing, filter changes, dithers and downloads, and frames
    /// accepted at each plan's own rate.
    Estimate(Estimate),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Overheads {
    /// Download and save after each exposure.
    pub per_exposure_seconds: f64,
    /// Slew, centre and rotate on a target change.
    pub target_change_seconds: f64,
    pub filter_change_seconds: f64,
    pub dither_seconds: f64,
}

impl Default for Overheads {
    fn default() -> Self {
        Self {
            per_exposure_seconds: 15.0,
            target_change_seconds: 90.0,
            filter_change_seconds: 10.0,
            dither_seconds: 15.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Estimate {
    pub overheads: Overheads,
    /// The share of frames each exposure plan, by id, has had accepted;
    /// a plan not listed counts every frame.
    pub acceptance: BTreeMap<i64, f64>,
}

/// One run of the planner.
pub struct Request<'a> {
    pub snapshot: &'a Snapshot,
    pub profile: NinaProfile,
    pub clock: Clock,
    /// Each night to plan, noon to noon, in order.
    pub nights: Vec<Interval>,
    /// Plan from here: the first night starts no earlier.
    pub start_ms: u64,
    /// When the rig may plan to image; `None` is always. Outside these
    /// windows the emulation waits, as Target Scheduler waits for a target.
    pub available: Option<&'a [Interval]>,
    pub mode: Mode,
}

/// Why nothing is shot over a stretch of a night.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GapReason {
    /// No template's twilight level allows imaging yet, or any more.
    Twilight,
    /// Outside the night's available windows.
    Unavailable,
    /// Every waiting candidate's plans fail Moon avoidance.
    Moon,
    /// The meridian flip's no-flip zone, or outside a meridian window.
    Meridian,
    /// Nothing is above its horizon and limits for its minimum time.
    BelowLimits,
    /// Every target is complete.
    Complete,
    /// Targets are owed, but none can be imaged before the night ends.
    NothingEligible,
    /// A Director rig's overrides hold it for the rest of the night. Target
    /// Scheduler's emulation never gives this.
    Held,
    /// A Director rig's overrides exclude every goal still owed. Target
    /// Scheduler's emulation never gives this.
    Excluded,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Gap {
    pub start_ms: u64,
    pub end_ms: u64,
    pub reason: GapReason,
    /// The target waited for, when there is one.
    pub waiting_for: Option<TargetRef>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TargetRef {
    pub project_id: i64,
    pub project_name: String,
    pub target_id: i64,
    pub target_name: String,
}

/// Consecutive exposures of one filter.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FilterRun {
    pub exposure_plan_id: i64,
    pub filter: String,
    pub exposure_seconds: f64,
    pub frames: u32,
    pub dithers: u32,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Where the Moon stands halfway through the run. The planner leaves it
    /// out; the server fills it in for the night view.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moon: Option<RunMoon>,
}

/// The Moon seen from the site at one instant.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct RunMoon {
    /// Above the horizon when positive.
    pub altitude_degrees: f64,
    /// The angle between the Moon and the run's target.
    pub separation_degrees: f64,
}

/// A stretch on one target.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Block {
    pub target: TargetRef,
    pub start_ms: u64,
    pub end_ms: u64,
    /// Whether the block starts with a slew and centre.
    pub slew: bool,
    pub runs: Vec<FilterRun>,
    /// Why the planner chose the target.
    pub decision: Decision,
}

/// How the planner chose a block's target.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Decision {
    /// `only`: the one target ready; `scored`: the best score of those
    /// ready; `continued`: the previous target, within its minimum time;
    /// `ranked`: a Director rig's highest-priority feasible goal.
    pub kind: DecisionKind,
    /// Every target the planner looked at, and where it stood.
    pub candidates: Vec<Candidate>,
    /// A Director rig's reason and the goal's priority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    Only,
    Scored,
    Continued,
    Ranked,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Candidate {
    pub target: TargetRef,
    /// `None` for the chosen target.
    pub rejected: Option<TargetReason>,
    pub score: Option<Score>,
    /// When a target set aside for now could start.
    pub could_start_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct NightSchedule {
    pub start_ms: u64,
    pub end_ms: u64,
    pub blocks: Vec<Block>,
    pub gaps: Vec<Gap>,
    pub frames: u32,
    pub exposure_hours: f64,
    /// Set when the planner could not plan the night: no sunset or sunrise.
    pub note: Option<String>,
}

/// One exposure plan's frames over the range.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PlanProgress {
    pub project_id: i64,
    pub target_id: i64,
    pub exposure_plan_id: i64,
    pub filter: String,
    pub desired: u32,
    pub accepted_before: f64,
    pub acquired_before: f64,
    /// Frames planned on each night of the range.
    pub planned: Vec<u32>,
    /// The night, by index, after which the plan owes nothing.
    pub completes_on: Option<usize>,
    pub complete_before: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Simulation {
    pub nights: Vec<NightSchedule>,
    pub progress: Vec<PlanProgress>,
}

fn ms(value: i64) -> u64 {
    value.max(0) as u64
}

/// Run the planner over the nights.
pub fn simulate(request: &Request) -> Simulation {
    let (sampling, estimate) = match &request.mode {
        Mode::Preview => (Sampling::preview(), None),
        Mode::Estimate(estimate) => (Sampling::live(), Some(estimate)),
    };
    let mut planner = Planner::new(
        request.snapshot,
        request.profile.clone(),
        request.clock,
        sampling,
    );
    let mut progress = initial_progress(&planner, request.nights.len());
    let mut nights = Vec::with_capacity(request.nights.len());
    for (index, span) in request.nights.iter().enumerate() {
        if index > 0 {
            planner.new_night();
        }
        let start = (span.start_ms.max(request.start_ms)) as i64;
        let end = span.end_ms as i64;
        let night = run_night(
            &mut planner,
            request,
            estimate,
            start,
            end,
            &mut progress,
            index,
        );
        nights.push(night);
        for entry in &mut progress {
            if entry.completes_on.is_none()
                && !entry.complete_before
                && owes_nothing(&planner, entry)
            {
                entry.completes_on = Some(index);
            }
        }
    }
    Simulation { nights, progress }
}

fn initial_progress(planner: &Planner, nights: usize) -> Vec<PlanProgress> {
    let mut progress = Vec::new();
    for target in &planner.targets {
        let project = &planner.projects[target.project];
        for exposure in &target.all {
            progress.push(PlanProgress {
                project_id: project.id,
                target_id: target.id,
                exposure_plan_id: exposure.plan.id,
                filter: exposure.filter.clone(),
                desired: exposure.plan.desired,
                accepted_before: exposure.accepted,
                acquired_before: exposure.acquired,
                planned: vec![0; nights],
                completes_on: None,
                complete_before: exposure.remaining(project.grader, planner.preferences()) <= 1e-9,
            });
        }
    }
    progress
}

fn owes_nothing(planner: &Planner, entry: &PlanProgress) -> bool {
    planner.targets.iter().any(|target| {
        target.id == entry.target_id
            && target.all.iter().any(|e| {
                e.plan.id == entry.exposure_plan_id
                    && e.remaining(
                        planner.projects[target.project].grader,
                        planner.preferences(),
                    ) <= 1e-9
            })
    })
}

fn target_ref(planner: &Planner, t: usize) -> TargetRef {
    let target = &planner.targets[t];
    let project = &planner.projects[target.project];
    TargetRef {
        project_id: project.id,
        project_name: project.name.clone(),
        target_id: target.id,
        target_name: target.name.clone(),
    }
}

/// The available window holding an instant, or the next one's start.
enum Clear {
    Until(i64),
    From(i64),
    Never,
}

fn clear_at(available: Option<&[Interval]>, at: i64) -> Clear {
    let Some(windows) = available else {
        return Clear::Until(i64::MAX);
    };
    let mut next: Option<i64> = None;
    for window in windows {
        let (start, end) = (window.start_ms as i64, window.end_ms as i64);
        if start <= at && at < end {
            return Clear::Until(end);
        }
        if start > at && next.is_none_or(|n| start < n) {
            next = Some(start);
        }
    }
    next.map_or(Clear::Never, Clear::From)
}

#[allow(clippy::too_many_arguments)]
fn run_night(
    planner: &mut Planner,
    request: &Request,
    estimate: Option<&Estimate>,
    start: i64,
    end: i64,
    progress: &mut [PlanProgress],
    night_index: usize,
) -> NightSchedule {
    let mut night = NightSchedule {
        start_ms: ms(start),
        end_ms: ms(end),
        blocks: Vec::new(),
        gaps: Vec::new(),
        frames: 0,
        exposure_hours: 0.0,
        note: None,
    };
    let mut at = start;
    let mut previous: Option<usize> = None;
    let mut last_filter: Option<String> = None;
    let mut calls = 0;
    while at < end && calls < MAX_CALLS_PER_NIGHT {
        calls += 1;
        // Outside availability the rig waits, as for a target.
        let window_end = match clear_at(request.available, at) {
            Clear::Until(until) => until,
            Clear::From(from) => {
                let until = from.min(end);
                push_unavailable(&mut night, planner, at, until);
                at = until;
                previous = None;
                continue;
            }
            Clear::Never => {
                push_unavailable(&mut night, planner, at, end);
                break;
            }
        };
        let plan = match planner.plan(at, previous) {
            Ok(plan) => plan,
            Err(NoNight) => {
                night.note = Some(
                    "The Sun does not set and rise this night, so Target Scheduler cannot plan it."
                        .into(),
                );
                break;
            }
        };
        match plan {
            Plan::Exposure {
                target,
                exposure,
                slew,
                dither,
                continued,
            } => {
                let seconds = planner.targets[target].all[exposure].seconds;
                let filter = planner.targets[target].all[exposure].filter.clone();
                let mut before = 0.0;
                let mut after = 0.0;
                if let Some(estimate) = estimate {
                    let overheads = &estimate.overheads;
                    if slew {
                        before += overheads.target_change_seconds;
                    }
                    if last_filter.as_deref() != Some(filter.as_str()) {
                        before += overheads.filter_change_seconds;
                    }
                    if dither {
                        before += overheads.dither_seconds;
                    }
                    after = overheads.per_exposure_seconds;
                }
                let finish = at + ((before + seconds + after) * 1000.0) as i64;
                if finish > window_end {
                    // The exposure would run into time the night is not
                    // clear: the rig waits for the next window instead.
                    let until = window_end.min(end);
                    push_unavailable(&mut night, planner, at, until);
                    at = until;
                    previous = None;
                    planner.clear_rejections();
                    continue;
                }
                if estimate.is_some()
                    && let Some(stop) = planner.targets[target].end
                    && at + (seconds * 1000.0) as i64 > stop
                    && stop > at
                {
                    // A live run's end-time trigger skips an exposure that
                    // would pass the target's window.
                    push_gap(
                        &mut night,
                        at,
                        stop.min(end),
                        GapReason::BelowLimits,
                        Some(target_ref(planner, target)),
                    );
                    at = stop.min(end);
                    planner.clear_rejections();
                    continue;
                }
                let decision = decide(planner, target, continued);
                record(
                    &mut night, planner, target, exposure, slew, dither, at, finish, decision,
                );
                if request.mode == Mode::Preview
                    && let Some(previous) = previous
                    && previous != target
                {
                    planner.reset_target(target);
                }
                let id = planner.targets[target].all[exposure].plan.id;
                let acceptance = estimate
                    .and_then(|e| e.acceptance.get(&id).copied())
                    .unwrap_or(1.0);
                planner.taken(target, exposure, acceptance);
                if let Some(entry) = progress
                    .iter_mut()
                    .find(|p| p.exposure_plan_id == id && p.target_id == planner.targets[target].id)
                {
                    entry.planned[night_index] += 1;
                }
                night.frames += 1;
                night.exposure_hours += seconds / 3600.0;
                last_filter = Some(filter);
                at = finish;
                previous = Some(target);
            }
            Plan::Wait {
                until,
                target,
                reason,
            } => {
                let until = until.min(end);
                let waiting = Some(target_ref(planner, target));
                let from = dusk_split(&mut night, planner, at, until, waiting.clone());
                let gap = classify_wait(planner, from, target, reason);
                push_gap(&mut night, from, until, gap, waiting);
                at = until.max(at + 1);
                previous = None;
            }
            Plan::Done => {
                let reason = match planner.done_reason(at) {
                    DoneReason::Complete => GapReason::Complete,
                    DoneReason::Twilight => GapReason::Twilight,
                    DoneReason::Moon => GapReason::Moon,
                    DoneReason::Meridian => GapReason::Meridian,
                    DoneReason::BelowLimits => GapReason::BelowLimits,
                    DoneReason::NothingEligible => GapReason::NothingEligible,
                };
                let from = if reason == GapReason::Complete {
                    at
                } else {
                    dusk_split(&mut night, planner, at, end, None)
                };
                push_gap(&mut night, from, end, reason, None);
                planner.clear_rejections();
                break;
            }
        }
        planner.clear_rejections();
    }
    night
}

/// Time the night is not marked clear: twilight where it is too light
/// anyway, and not available where it is dark enough to image.
fn push_unavailable(night: &mut NightSchedule, planner: &mut Planner, from: i64, to: i64) {
    let mut start = from;
    let mut dark = planner.twilight_allows_any(from);
    let mut at = from;
    while at < to {
        let next = (at + 60_000).min(to);
        let now_dark = next < to && planner.twilight_allows_any(next);
        if now_dark != dark || next == to {
            let reason = if dark {
                GapReason::Unavailable
            } else {
                GapReason::Twilight
            };
            push_gap(night, start, next, reason, None);
            start = next;
            dark = now_dark;
        }
        at = next;
    }
}

/// A gap that starts before twilight allows any owed exposure: the part
/// before is twilight's. Returns where the rest starts.
fn dusk_split(
    night: &mut NightSchedule,
    planner: &mut Planner,
    from: i64,
    to: i64,
    waiting_for: Option<TargetRef>,
) -> i64 {
    if planner.twilight_allows_any(from) {
        return from;
    }
    let opens = planner.twilight_opens(from, to).unwrap_or(to);
    push_gap(night, from, opens, GapReason::Twilight, waiting_for);
    opens
}

fn classify_wait(
    planner: &mut Planner,
    at: i64,
    target: usize,
    reason: Option<TargetReason>,
) -> GapReason {
    if !planner.twilight_allows_any(at) {
        return GapReason::Twilight;
    }
    match planner.targets[target].blocked.or(reason) {
        Some(TargetReason::MoonAvoidance) => GapReason::Moon,
        Some(TargetReason::BeforeMeridianWindow) => GapReason::Meridian,
        Some(TargetReason::MeridianFlipClipped) if planner.above_horizon(target, at) => {
            GapReason::Meridian
        }
        Some(_) => GapReason::BelowLimits,
        None => GapReason::NothingEligible,
    }
}

fn push_gap(
    night: &mut NightSchedule,
    start: i64,
    end: i64,
    reason: GapReason,
    waiting_for: Option<TargetRef>,
) {
    if end <= start {
        return;
    }
    if let Some(last) = night.gaps.last_mut()
        && last.end_ms == ms(start)
        && last.reason == reason
        && last.waiting_for == waiting_for
    {
        last.end_ms = ms(end);
        return;
    }
    night.gaps.push(Gap {
        start_ms: ms(start),
        end_ms: ms(end),
        reason,
        waiting_for,
    });
}

fn decide(planner: &Planner, chosen: usize, continued: bool) -> Decision {
    if continued {
        return Decision {
            kind: DecisionKind::Continued,
            candidates: Vec::new(),
            note: None,
        };
    }
    let mut candidates = Vec::new();
    let mut scored = false;
    for project in &planner.projects {
        for &t in &project.targets {
            let target = &planner.targets[t];
            if target.rejected == Some(TargetReason::Complete) {
                continue;
            }
            scored |= target.score.is_some();
            candidates.push(Candidate {
                target: target_ref(planner, t),
                rejected: if t == chosen {
                    None
                } else {
                    target.rejected.or(Some(if project.rejected_any() {
                        TargetReason::Project
                    } else {
                        TargetReason::NotReady
                    }))
                },
                score: target.score.clone(),
                could_start_ms: match target.rejected {
                    Some(
                        TargetReason::NotYetVisible
                        | TargetReason::BeforeMeridianWindow
                        | TargetReason::MeridianFlipClipped
                        | TargetReason::MaxAltitude,
                    ) => target.start.map(ms),
                    _ => None,
                },
            });
        }
    }
    Decision {
        kind: if scored {
            DecisionKind::Scored
        } else {
            DecisionKind::Only
        },
        candidates,
        note: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn record(
    night: &mut NightSchedule,
    planner: &Planner,
    t: usize,
    exposure: usize,
    slew: bool,
    dither: bool,
    start: i64,
    finish: i64,
    decision: Decision,
) {
    let target = &planner.targets[t];
    let plan = &target.all[exposure];
    let reference = target_ref(planner, t);
    let continues = night
        .blocks
        .last()
        .is_some_and(|block| block.target == reference && block.end_ms == ms(start) && !slew);
    if !continues {
        night.blocks.push(Block {
            target: reference,
            start_ms: ms(start),
            end_ms: ms(finish),
            slew,
            runs: Vec::new(),
            decision,
        });
    }
    let Some(block) = night.blocks.last_mut() else {
        return;
    };
    block.end_ms = ms(finish);
    match block.runs.last_mut() {
        Some(run) if run.exposure_plan_id == plan.plan.id && run.end_ms == ms(start) => {
            run.frames += 1;
            run.dithers += u32::from(dither);
            run.end_ms = ms(finish);
        }
        _ => block.runs.push(FilterRun {
            exposure_plan_id: plan.plan.id,
            filter: plan.filter.clone(),
            exposure_seconds: plan.seconds,
            frames: 1,
            dithers: u32::from(dither),
            start_ms: ms(start),
            end_ms: ms(finish),
            moon: None,
        }),
    }
}
