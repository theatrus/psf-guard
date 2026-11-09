// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! One call of Target Scheduler's planner: stay with the previous target,
//! or filter every target for completion, visibility, the Moon and
//! twilight, pick an exposure for each target ready now, score them, and
//! return the winner, a wait until the next target, or nothing.

use std::collections::HashMap;

use psf_guard_director_core::moon::MoonPolicy;
use psf_guard_director_core::visibility::{Horizon, Site};
use serde::Serialize;

use crate::astro::{Frame, DAY_MS};
use crate::select::{Choices, DitherCount, Selector};
use crate::snapshot::{
    throttle_at, ExposurePlan, ExposureTemplate, Preferences, Project, ProjectPriority, Snapshot,
    TwilightLevel,
};
use crate::twilight::{Clock, Twilight};
use crate::visibility::{clip_meridian_window, seconds, FlipZone, Visibility};

/// Why Target Scheduler set a target aside, in its own words.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetReason {
    Complete,
    NeverRises,
    NotVisible,
    NotYetVisible,
    MaxAltitude,
    MeridianWindowClipped,
    BeforeMeridianWindow,
    MeridianFlipClipped,
    /// All exposure plans rejected for twilight.
    Twilight,
    /// All exposure plans rejected for Moon avoidance.
    MoonAvoidance,
    LowerScore,
    /// All exposure plans rejected, for different reasons.
    AllExposurePlans,
    /// Its project was set aside.
    Project,
    /// Visible, but its window does not start within two samples of now.
    NotReady,
}

impl TargetReason {
    /// The reasons the planner may wait out.
    fn waitable(self) -> bool {
        matches!(
            self,
            Self::NotYetVisible
                | Self::MoonAvoidance
                | Self::BeforeMeridianWindow
                | Self::MeridianFlipClipped
                | Self::MaxAltitude
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExposureReason {
    Complete,
    MoonAvoidance,
    Twilight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectReason {
    Complete,
    AllTargets,
}

/// A scoring rule, with Target Scheduler's name and default weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Rule {
    MeridianFlipPenalty,
    MeridianWindowPriority,
    MosaicCompletion,
    PercentComplete,
    ProjectPriority,
    SettingSoonest,
    SmartExposureOrder,
    TargetSwitchPenalty,
}

impl Rule {
    pub const ALL: [Rule; 8] = [
        Rule::MeridianFlipPenalty,
        Rule::MeridianWindowPriority,
        Rule::MosaicCompletion,
        Rule::PercentComplete,
        Rule::ProjectPriority,
        Rule::SettingSoonest,
        Rule::SmartExposureOrder,
        Rule::TargetSwitchPenalty,
    ];

    /// The `ruleweight.name` Target Scheduler stores.
    pub fn name(self) -> &'static str {
        match self {
            Self::MeridianFlipPenalty => "Meridian Flip Penalty",
            Self::MeridianWindowPriority => "Meridian Window Priority",
            Self::MosaicCompletion => "Mosaic Completion",
            Self::PercentComplete => "Percent Complete",
            Self::ProjectPriority => "Project Priority",
            Self::SettingSoonest => "Setting Soonest",
            Self::SmartExposureOrder => "Smart Exposure Order",
            Self::TargetSwitchPenalty => "Target Switch Penalty",
        }
    }

    pub fn default_weight(self) -> f64 {
        match self {
            Self::MeridianFlipPenalty | Self::MosaicCompletion | Self::SmartExposureOrder => 0.0,
            Self::MeridianWindowPriority => 75.0,
            Self::PercentComplete | Self::ProjectPriority | Self::SettingSoonest => 50.0,
            Self::TargetSwitchPenalty => 67.0,
        }
    }
}

/// One rule's part in a target's score.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RuleScore {
    pub rule: Rule,
    /// 0 to 1.
    pub raw: f64,
    /// 0 to 100.
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Score {
    /// Σ weight ÷ 100 × raw.
    pub total: f64,
    pub rules: Vec<RuleScore>,
}

/// Sampling: the live run's, or the three times coarser Scheduler Preview's.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Sampling {
    pub(crate) interval_ms: i64,
    pub(crate) future_step_ms: i64,
    pub(crate) preview: bool,
}

impl Sampling {
    pub(crate) fn live() -> Self {
        Self {
            interval_ms: 10_000,
            future_step_ms: 60_000,
            preview: false,
        }
    }

    pub(crate) fn preview() -> Self {
        Self {
            interval_ms: 30_000,
            future_step_ms: 180_000,
            preview: true,
        }
    }
}

pub(crate) struct Exposure {
    pub(crate) plan: ExposurePlan,
    pub(crate) filter: String,
    pub(crate) seconds: f64,
    twilight_level: TwilightLevel,
    twilight_offset_ms: i64,
    dither_every: i64,
    moon: MoonPolicy,
    /// Counts as the run goes; fractional when frames count at an
    /// acceptance ratio.
    pub(crate) acquired: f64,
    pub(crate) accepted: f64,
    pub(crate) rejected: Option<ExposureReason>,
    moon_score: f64,
    pub(crate) pre_dither: bool,
}

pub(crate) struct Target {
    pub(crate) project: usize,
    pub(crate) id: i64,
    pub(crate) name: String,
    ra_hours: f64,
    dec_degrees: f64,
    rotation_degrees: f64,
    pub(crate) all: Vec<Exposure>,
    /// Target Scheduler's `ExposurePlans`: the working plans, by index into `all`.
    plans: Vec<usize>,
    /// `CompletedExposurePlans`.
    completed: Vec<usize>,
    selector: Selector,
    pub(crate) dithers: DitherCount,
    pub(crate) rejected: Option<TargetReason>,
    pub(crate) start: Option<i64>,
    pub(crate) end: Option<i64>,
    minimum_span_end: i64,
    bonus_span_end: i64,
    culmination: Option<i64>,
    pub(crate) selected: Option<usize>,
    pub(crate) score: Option<Score>,
    /// What last stopped the wait search finding a start for the target:
    /// the Moon or twilight, before it left the sky.
    pub(crate) blocked: Option<TargetReason>,
    visibility: HashMap<i64, Option<Visibility>>,
}

pub(crate) struct PlanningProject {
    pub(crate) id: i64,
    pub(crate) name: String,
    priority: ProjectPriority,
    minimum_time_ms: i64,
    minimum_altitude: f64,
    maximum_altitude: f64,
    use_custom_horizon: bool,
    horizon_offset: f64,
    meridian_window_ms: i64,
    dither_every: u32,
    pub(crate) grader: bool,
    mosaic: bool,
    smart: bool,
    weights: [f64; 8],
    pub(crate) targets: Vec<usize>,
    rejected: Option<ProjectReason>,
}

/// The rig's N.I.N.A. settings the planner reads.
#[derive(Clone, Debug)]
pub struct NinaProfile {
    pub site: Site,
    /// The profile's custom horizon, if any.
    pub horizon: Horizon,
    /// Meridian flip: minutes paused before the meridian (0 turns the
    /// no-flip zone off) and minutes after it.
    pub pause_before_meridian_minutes: f64,
    pub minutes_after_meridian: f64,
}

/// Why a night has no more work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DoneReason {
    Complete,
    Twilight,
    Moon,
    Meridian,
    BelowLimits,
    NothingEligible,
}

/// What one planner call decided.
pub(crate) enum Plan {
    /// Take one exposure of a target's plan.
    Exposure {
        target: usize,
        exposure: usize,
        slew: bool,
        dither: bool,
        /// True when the call stayed with the previous target without
        /// planning again.
        continued: bool,
    },
    /// Wait until a target can be imaged.
    Wait {
        until: i64,
        target: usize,
        reason: Option<TargetReason>,
    },
    /// Nothing more tonight.
    Done,
}

/// What the planner carries between calls.
pub(crate) struct Planner {
    pub(crate) clock: Clock,
    nina: NinaProfile,
    preferences: Preferences,
    pub(crate) sampling: Sampling,
    pub(crate) projects: Vec<PlanningProject>,
    pub(crate) targets: Vec<Target>,
    frames: HashMap<i64, Frame>,
    twilights: HashMap<i64, Twilight>,
    /// The Moon at the instant last tested, which every exposure's Moon
    /// avoidance then shares.
    moon: Option<MoonAt>,
}

/// The Moon at one instant, as Target Scheduler's Moon avoidance reads it,
/// each part computed when a test first needs it.
#[derive(Clone, Copy)]
struct MoonAt {
    at: i64,
    /// Where the site sees it, of date, and its altitude.
    seen: Option<(crate::astro::OfDate, f64)>,
    /// Its age in days.
    age_days: Option<Option<f64>>,
}

impl MoonAt {
    fn new(at: i64) -> Self {
        Self {
            at,
            seen: None,
            age_days: None,
        }
    }

    fn seen(&mut self, frame: &Frame) -> (crate::astro::OfDate, f64) {
        let at = self.at;
        *self.seen.get_or_insert_with(|| {
            let seen = frame.moon_seen(at);
            (seen, frame.altitude_of(seen, at))
        })
    }
}

/// The planner could not decide: a night without sunset or sunrise.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct NoNight;

impl Exposure {
    fn new(plan: &ExposurePlan, template: &ExposureTemplate) -> Self {
        Self {
            plan: plan.clone(),
            filter: template.filter_name.clone(),
            seconds: plan.exposure_seconds(template),
            twilight_level: template.twilight_level,
            twilight_offset_ms: template.twilight_offset_minutes * 60_000,
            dither_every: template.dither_every,
            moon: template.moon.clone(),
            acquired: f64::from(plan.acquired),
            accepted: f64::from(plan.accepted),
            rejected: None,
            moon_score: 0.0,
            pre_dither: false,
        }
    }

    /// Frames still owed, as the completion filter counts them.
    pub(crate) fn remaining(&self, grader: bool, preferences: &Preferences) -> f64 {
        let desired = f64::from(self.plan.desired);
        if grader {
            (desired - self.accepted).max(0.0)
        } else {
            (f64::from(throttle_at(
                self.plan.desired,
                preferences.exposure_throttle_percent,
            )) - self.acquired)
                .max(0.0)
        }
    }

    /// Percent complete, 0 to 100, as the Percent Complete rule and the
    /// working list count it.
    pub(crate) fn percent_complete(&self, grader: bool, preferences: &Preferences) -> f64 {
        let desired = f64::from(self.plan.desired);
        if grader {
            if desired == 0.0 {
                return 0.0;
            }
            let acquired = self.acquired / desired * 100.0;
            let provisional = preferences.delay_grading_percent > 0.0
                && acquired < preferences.delay_grading_percent;
            let percent = if provisional {
                acquired
            } else {
                self.accepted / desired * 100.0
            };
            percent.min(100.0)
        } else {
            if self.acquired == 0.0 {
                return 0.0;
            }
            let at = f64::from(throttle_at(
                self.plan.desired,
                preferences.exposure_throttle_percent,
            ));
            (self.acquired / at * 100.0).min(100.0)
        }
    }

    fn incomplete(&self, grader: bool, preferences: &Preferences) -> bool {
        self.percent_complete(grader, preferences) < 100.0
    }

    /// The Moon avoidance score smart selection ranks by: fixed by the
    /// template, never by the Moon.
    fn score_moon(&self) -> f64 {
        if !self.moon.enabled {
            0.0
        } else if self.moon_down() {
            1.0
        } else {
            self.moon.separation_degrees * self.moon.width_days / (180.0 * 14.0)
        }
    }

    fn moon_down(&self) -> bool {
        self.moon.moon_down
    }
}

impl Planner {
    pub(crate) fn new(
        snapshot: &Snapshot,
        nina: NinaProfile,
        clock: Clock,
        sampling: Sampling,
    ) -> Self {
        let mut projects = Vec::new();
        let mut targets = Vec::new();
        let preferences = snapshot.preferences.clone();
        for project in snapshot
            .projects
            .iter()
            .filter(|p| p.state == crate::snapshot::ProjectState::Active)
        {
            let index = projects.len();
            let mut planning = planning_project(project);
            for target in project.targets.iter().filter(|t| t.active) {
                let all: Vec<Exposure> = target
                    .exposure_plans
                    .iter()
                    .filter(|p| p.enabled)
                    .filter_map(|p| {
                        snapshot
                            .templates
                            .iter()
                            .find(|t| t.id == p.template_id)
                            .map(|t| Exposure::new(p, t))
                    })
                    .collect();
                let selector = Selector::new(
                    project.smart_exposure_order,
                    project.filter_switch_frequency,
                    &target.override_order,
                    &target.filter_cadence,
                    all.len(),
                );
                let mut planning_target = Target {
                    project: index,
                    id: target.id,
                    name: target.name.clone(),
                    ra_hours: target.position.ra_degrees / 15.0,
                    dec_degrees: target.position.dec_degrees,
                    rotation_degrees: target.rotation_degrees,
                    all,
                    plans: Vec::new(),
                    completed: Vec::new(),
                    selector,
                    dithers: DitherCount::default(),
                    rejected: None,
                    start: None,
                    end: None,
                    minimum_span_end: 0,
                    bonus_span_end: 0,
                    culmination: None,
                    selected: None,
                    score: None,
                    blocked: None,
                    visibility: HashMap::new(),
                };
                // With an override order, only the plans it refers to, in the
                // order of their first mention.
                let active: Vec<usize> = if target.override_order.is_empty() {
                    (0..planning_target.all.len()).collect()
                } else {
                    let mut seen = Vec::new();
                    for plan in planning_target.selector.referenced() {
                        if !seen.contains(&plan) {
                            seen.push(plan);
                        }
                    }
                    seen
                };
                split(&mut planning_target, &active, planning.grader, &preferences);
                planning.targets.push(targets.len());
                targets.push(planning_target);
            }
            projects.push(planning);
        }
        Self {
            clock,
            nina,
            preferences,
            sampling,
            projects,
            targets,
            frames: HashMap::new(),
            twilights: HashMap::new(),
            moon: None,
        }
    }

    pub(crate) fn preferences(&self) -> &Preferences {
        &self.preferences
    }

    fn frame(&mut self, noon: i64) -> &Frame {
        let site = self.nina.site;
        self.frames
            .entry(noon)
            .or_insert_with(|| Frame::new(site, noon))
    }

    fn twilight_at_noon(&mut self, noon: i64) -> Twilight {
        if let Some(found) = self.twilights.get(&noon) {
            return found.clone();
        }
        let twilight = Twilight::new(self.frame(noon), noon);
        self.twilights.insert(noon, twilight.clone());
        twilight
    }

    /// The night that applies at an instant: tonight from noon until
    /// sunrise, last night from midnight until this morning's sunrise.
    pub(crate) fn night_at(&mut self, at: i64) -> Result<Twilight, NoNight> {
        let noon = self.clock.noon(at);
        let tonight = self.twilight_at_noon(noon);
        let (sunset, sunrise) = tonight.civil.ok_or(NoNight)?;
        if (noon <= at && at < sunset) || (sunset <= at && at < sunrise) {
            return Ok(tonight);
        }
        let last = self.twilight_at_noon(noon - DAY_MS);
        if last.civil.is_some_and(|(_, rise)| rise <= at && at < noon) {
            return Ok(tonight);
        }
        Ok(last)
    }

    fn horizon_at(&self, project: usize, azimuth: f64) -> f64 {
        let project = &self.projects[project];
        if !project.use_custom_horizon {
            return project.minimum_altitude;
        }
        match self.nina.horizon.altitude(azimuth) {
            Ok(Some(custom)) => (custom + project.horizon_offset)
                .max(project.minimum_altitude)
                .min(90.0),
            _ => project.minimum_altitude,
        }
    }

    fn visibility(&mut self, target: usize, night: &Twilight) -> Option<Visibility> {
        if let Some(found) = self.targets[target].visibility.get(&night.noon_ms) {
            return found.clone();
        }
        let project = self.targets[target].project;
        let (ra, dec) = (
            self.targets[target].ra_hours,
            self.targets[target].dec_degrees,
        );
        let maximum = self.projects[project].maximum_altitude;
        let interval = self.sampling.interval_ms;
        let clock = self.clock;
        let frame = self.frame(night.noon_ms).clone();
        let horizon = |azimuth: f64| self.horizon_at(project, azimuth);
        let found = Visibility::new(&frame, clock, night, interval, ra, dec, maximum, &horizon);
        self.targets[target]
            .visibility
            .insert(night.noon_ms, found.clone());
        found
    }

    fn flip_zone(&self) -> Option<FlipZone> {
        (self.nina.pause_before_meridian_minutes > 0.0).then_some(FlipZone {
            pause_before_ms: (self.nina.pause_before_meridian_minutes * 60_000.0) as i64,
            after_ms: (self.nina.minutes_after_meridian * 60_000.0) as i64,
        })
    }

    /// Clear every rejection and score, as the preview does after each plan.
    pub(crate) fn clear_rejections(&mut self) {
        for project in &mut self.projects {
            project.rejected = None;
        }
        for target in &mut self.targets {
            target.rejected = None;
            target.score = None;
            for &plan in &target.plans {
                target.all[plan].rejected = None;
                target.all[plan].moon_score = 0.0;
            }
        }
    }

    /// Rebuild targets from their counts, as a live run does when it loads
    /// the database on every call: fresh working lists, no rejections and
    /// no window yet. `keep` keeps its window, for staying with it.
    pub(crate) fn reload(&mut self, keep: Option<usize>) {
        let preferences = self.preferences.clone();
        for index in 0..self.targets.len() {
            if Some(index) != keep {
                let target = &mut self.targets[index];
                target.start = None;
                target.end = None;
                target.culmination = None;
                target.selected = None;
            }
            let grader = self.projects[self.targets[index].project].grader;
            let target = &mut self.targets[index];
            let active: Vec<usize> = if target.selector.is_override() {
                let mut seen = Vec::new();
                for plan in target.selector.referenced() {
                    if !seen.contains(&plan) {
                        seen.push(plan);
                    }
                }
                seen
            } else {
                (0..target.all.len()).collect()
            };
            for plan in &mut target.all {
                plan.rejected = None;
            }
            split(target, &active, grader, &preferences);
        }
    }

    /// Plan at `at`, after `previous`.
    pub(crate) fn plan(&mut self, at: i64, previous: Option<usize>) -> Result<Plan, NoNight> {
        if !self.sampling.preview {
            self.reload(previous);
        }
        if let Some(previous) = previous
            && self.can_continue(at, previous)?
        {
            let exposure = self.targets[previous].selected.unwrap_or_default();
            let dither = self.targets[previous].all[exposure].pre_dither;
            return Ok(Plan::Exposure {
                target: previous,
                exposure,
                slew: false,
                dither,
                continued: true,
            });
        }
        if !self.sampling.preview {
            // The full run plans with freshly loaded targets.
            self.reload(None);
        }
        self.filter_incomplete();
        self.propagate();
        self.filter_visibility(at)?;
        self.propagate();
        self.filter_moon(at)?;
        self.propagate();
        self.filter_twilight(at)?;
        self.propagate();
        let mut ready: Vec<usize> = self
            .projects
            .iter()
            .filter(|p| p.rejected.is_none())
            .flat_map(|p| p.targets.iter().copied())
            .filter(|&t| {
                let target = &self.targets[t];
                target.rejected.is_none()
                    && target
                        .start
                        .is_some_and(|s| (at - s).abs() <= 2 * self.sampling.interval_ms)
            })
            .collect();
        if !ready.is_empty() {
            for &t in &ready {
                self.select_exposure(t);
            }
            ready.retain(|&t| self.targets[t].selected.is_some());
            if ready.is_empty() {
                // Target Scheduler throws here and the run ends.
                return Ok(Plan::Done);
            }
            let chosen = if ready.len() == 1 {
                ready[0]
            } else {
                self.score(at, previous, &ready)
            };
            let exposure = self.targets[chosen].selected.unwrap_or_default();
            let slew = previous.is_none_or(|p| !self.same_target(chosen, p));
            let dither = self.targets[chosen].all[exposure].pre_dither;
            if let Some(previous) = previous
                && self.targets[chosen].id != self.targets[previous].id
                && !self.sampling.preview
            {
                // A live run forgets the target it leaves.
                self.targets[previous].dithers.clear();
            }
            return Ok(Plan::Exposure {
                target: chosen,
                exposure,
                slew,
                dither,
                continued: false,
            });
        }
        match self.next_possible(at)? {
            Some((target, until, reason)) => Ok(Plan::Wait {
                until,
                target,
                reason,
            }),
            None => Ok(Plan::Done),
        }
    }

    /// Target Scheduler's target equality: name, coordinates and rotation.
    fn same_target(&self, a: usize, b: usize) -> bool {
        let (a, b) = (&self.targets[a], &self.targets[b]);
        a.name == b.name
            && a.ra_hours == b.ra_hours
            && a.dec_degrees == b.dec_degrees
            && a.rotation_degrees == b.rotation_degrees
    }

    fn can_continue(&mut self, at: i64, previous: usize) -> Result<bool, NoNight> {
        let preferences = self.preferences.clone();
        let grader = self.projects[self.targets[previous].project].grader;
        {
            // Rebuild the working list from every enabled plan: this puts
            // back plans an override order leaves out.
            let target = &mut self.targets[previous];
            let all: Vec<usize> = (0..target.all.len()).collect();
            for plan in &mut target.all {
                plan.rejected = None;
            }
            split(target, &all, grader, &preferences);
            if target.selector.is_override() {
                let referenced = target.selector.referenced();
                for (index, plan) in target.all.iter_mut().enumerate() {
                    if !referenced.contains(&index) {
                        plan.rejected = Some(ExposureReason::Complete);
                    }
                }
            }
            if target.plans.is_empty() {
                return Ok(false);
            }
        }
        self.moon_filter_target(at, previous);
        if self.targets[previous]
            .plans
            .iter()
            .all(|&p| self.targets[previous].all[p].rejected.is_some())
        {
            return Ok(false);
        }
        let minimum = self.projects[self.targets[previous].project].minimum_time_ms;
        {
            let target = &mut self.targets[previous];
            if let Some(end) = target.end
                && seconds(target.minimum_span_end, end) < minimum / 1000
            {
                target.bonus_span_end = end;
            }
        }
        let Some(next) = self.choose(previous) else {
            return Ok(false);
        };
        let night = self.night_at(at)?;
        let Some(level) = night.level(at) else {
            // Target Scheduler throws in daylight; nothing continues.
            return Ok(false);
        };
        if twilight_rejects(&self.targets[previous].all[next], at, &night, level) {
            return Ok(false);
        }
        let target = &mut self.targets[previous];
        if at + (target.all[next].seconds * 1000.0) as i64 > target.bonus_span_end {
            return Ok(false);
        }
        target.selected = Some(next);
        Ok(true)
    }

    fn filter_incomplete(&mut self) {
        let preferences = self.preferences.clone();
        for p in 0..self.projects.len() {
            let grader = self.projects[p].grader;
            let mut any = false;
            for &t in &self.projects[p].targets {
                let target = &mut self.targets[t];
                for &plan in &target.plans {
                    if target.all[plan].remaining(grader, &preferences) <= 1e-9 {
                        target.all[plan].rejected = Some(ExposureReason::Complete);
                    } else {
                        any = true;
                    }
                }
            }
            if !any {
                self.projects[p].rejected = Some(ProjectReason::Complete);
                for &t in &self.projects[p].targets {
                    self.targets[t].rejected = Some(TargetReason::Complete);
                }
            }
        }
    }

    fn propagate(&mut self) {
        for p in 0..self.projects.len() {
            if self.projects[p].rejected.is_some() {
                continue;
            }
            for &t in &self.projects[p].targets {
                let target = &mut self.targets[t];
                if target.rejected.is_some() {
                    continue;
                }
                let reasons: Vec<Option<ExposureReason>> = target
                    .plans
                    .iter()
                    .map(|&i| target.all[i].rejected)
                    .collect();
                if reasons.iter().all(Option::is_some) {
                    let all = |reason| reasons.iter().all(|r| *r == Some(reason));
                    target.rejected = Some(if all(ExposureReason::Complete) {
                        TargetReason::Complete
                    } else if all(ExposureReason::Twilight) {
                        TargetReason::Twilight
                    } else if all(ExposureReason::MoonAvoidance) {
                        TargetReason::MoonAvoidance
                    } else {
                        TargetReason::AllExposurePlans
                    });
                }
            }
            if self.projects[p]
                .targets
                .iter()
                .all(|&t| self.targets[t].rejected.is_some())
            {
                self.projects[p].rejected = Some(ProjectReason::AllTargets);
            }
        }
    }

    fn filter_visibility(&mut self, at: i64) -> Result<(), NoNight> {
        let night = self.night_at(at)?;
        for p in 0..self.projects.len() {
            if self.projects[p].rejected.is_some() {
                continue;
            }
            for t in self.projects[p].targets.clone() {
                self.visible(at, t, &night);
            }
        }
        Ok(())
    }

    /// The span a target's working exposures allow, by their twilight
    /// levels and offsets.
    fn twilight_span(&self, target: usize, night: &Twilight) -> Option<(i64, i64)> {
        let target = &self.targets[target];
        let grader = self.projects[target.project].grader;
        let mut span: Option<(i64, i64)> = None;
        for &plan in &target.plans {
            let exposure = &target.all[plan];
            if exposure.rejected.is_some() || !exposure.incomplete(grader, &self.preferences) {
                continue;
            }
            if let Some((start, end)) = night.span(exposure.twilight_level) {
                let part = (
                    start + exposure.twilight_offset_ms,
                    end - exposure.twilight_offset_ms,
                );
                span = Some(match span {
                    None => part,
                    Some((s, e)) => (s.min(part.0), e.max(part.1)),
                });
            }
        }
        span
    }

    /// Target Scheduler's visibility test at `at`; true for a candidate.
    fn visible(&mut self, at: i64, t: usize, night: &Twilight) -> bool {
        if self.targets[t].rejected.is_some() {
            return false;
        }
        let latitude = self.nina.site.latitude_degrees;
        let difference = self.targets[t].dec_degrees - latitude;
        if !(-90.0..=90.0).contains(&difference) {
            self.targets[t].rejected = Some(TargetReason::NeverRises);
            return false;
        }
        let Some(span) = self.twilight_span(t, night) else {
            self.targets[t].rejected = Some(TargetReason::AllExposurePlans);
            return false;
        };
        let Some(seen) = self.visibility(t, night) else {
            self.targets[t].rejected = Some(TargetReason::NotVisible);
            return false;
        };
        if !seen.imaging_possible {
            self.targets[t].rejected = Some(TargetReason::NotVisible);
            return false;
        }
        let project = &self.projects[self.targets[t].project];
        let minimum = project.minimum_time_ms;
        let Some((mut start, mut end)) = seen.next_visible_for(at, span, minimum / 1000) else {
            self.targets[t].rejected = Some(TargetReason::NotVisible);
            return false;
        };
        let transit = seen.transit;
        let mut window = false;
        if project.meridian_window_ms > 0 {
            let Some((s, e)) =
                clip_meridian_window(start, transit, end, project.meridian_window_ms)
            else {
                self.targets[t].rejected = Some(TargetReason::MeridianWindowClipped);
                return false;
            };
            window = true;
            (start, end) = (s, e);
            if seconds(start, end) < minimum / 1000 {
                self.targets[t].rejected = Some(TargetReason::NotVisible);
                return false;
            }
        }
        if project.maximum_altitude != 0.0 {
            match seen.clip_maximum(start, end) {
                None => {
                    self.targets[t].start = Some(seen.next_safe_start(start, end));
                    self.targets[t].rejected = Some(TargetReason::MaxAltitude);
                    return false;
                }
                Some((s, e)) => {
                    (start, end) = (s, e);
                    if seconds(s, e) < minimum / 1000 {
                        // Target Scheduler moves the start to the end of the
                        // safe span.
                        start = e;
                        end = start + minimum;
                    }
                }
            }
        }
        let mut flip = false;
        if let Some(zone) = self.flip_zone() {
            let Some((s, e)) = zone.clip(start, transit, end) else {
                self.targets[t].rejected = Some(TargetReason::MeridianFlipClipped);
                return false;
            };
            flip = true;
            if seconds(s, e) < minimum / 1000 {
                if let Some(transit) = transit {
                    start = zone.safe_after(transit);
                }
            } else {
                (start, end) = (s, e);
            }
        }
        let target = &mut self.targets[t];
        let actual = at.max(start);
        if actual > at {
            target.start = Some(actual);
            target.end = Some(end);
            target.rejected = Some(if window {
                TargetReason::BeforeMeridianWindow
            } else if flip {
                TargetReason::MeridianFlipClipped
            } else {
                TargetReason::NotYetVisible
            });
            return false;
        }
        target.start = Some(start);
        target.end = Some(end);
        target.minimum_span_end = at + minimum;
        target.bonus_span_end = at + minimum;
        target.culmination = transit;
        true
    }

    fn filter_moon(&mut self, at: i64) -> Result<(), NoNight> {
        for p in 0..self.projects.len() {
            if self.projects[p].rejected.is_some() {
                continue;
            }
            for t in self.projects[p].targets.clone() {
                self.moon_filter_target(at, t);
            }
        }
        Ok(())
    }

    fn moon_filter_target(&mut self, at: i64, t: usize) {
        if self.targets[t]
            .rejected
            .is_some_and(|r| r != TargetReason::NotYetVisible)
        {
            return;
        }
        let grader = self.projects[self.targets[t].project].grader;
        let preferences = self.preferences.clone();
        let noon = self.clock.noon(at);
        let frame = self.frame(noon).clone();
        let mut moon = match self.moon.take() {
            Some(moon) if moon.at == at => moon,
            _ => MoonAt::new(at),
        };
        let target = &mut self.targets[t];
        let (ra, dec) = (target.ra_hours, target.dec_degrees);
        for plan in target.plans.clone() {
            let exposure = &mut target.all[plan];
            if !exposure.incomplete(grader, &preferences) {
                continue;
            }
            if moon_rejects(&frame, &mut moon, exposure, at, ra, dec) {
                exposure.rejected = Some(ExposureReason::MoonAvoidance);
            }
        }
        self.moon = Some(moon);
    }

    fn filter_twilight(&mut self, at: i64) -> Result<(), NoNight> {
        let night = self.night_at(at)?;
        let level = night.level(at);
        let preferences = self.preferences.clone();
        for p in 0..self.projects.len() {
            if self.projects[p].rejected.is_some() {
                continue;
            }
            let grader = self.projects[p].grader;
            for &t in &self.projects[p].targets {
                let target = &mut self.targets[t];
                if target.rejected.is_some() {
                    continue;
                }
                for plan in target.plans.clone() {
                    let exposure = &mut target.all[plan];
                    if exposure.rejected.is_some() || !exposure.incomplete(grader, &preferences) {
                        continue;
                    }
                    let rejected = match level {
                        None => true,
                        Some(level) => twilight_rejects(exposure, at, &night, level),
                    };
                    if rejected {
                        exposure.rejected = Some(ExposureReason::Twilight);
                    }
                }
            }
        }
        Ok(())
    }

    /// Choose the target's next exposure and set whether a dither comes
    /// first. `None` when its selector finds nothing.
    fn choose(&mut self, t: usize) -> Option<usize> {
        let project = &self.projects[self.targets[t].project];
        let project_dither = project.dither_every;
        let target = &mut self.targets[t];
        let rejected: Vec<bool> = target.all.iter().map(|e| e.rejected.is_some()).collect();
        let scores: Vec<f64> = target.all.iter().map(|e| e.moon_score).collect();
        let plans = target.plans.clone();
        let selected = target.selector.select(&Choices {
            working: &plans,
            rejected: &|i| rejected[i],
            moon_score: &|i| scores[i],
        })?;
        let exposure = &target.all[selected.plan];
        let dither = match selected.order_dither {
            Some(dither) => dither,
            None => {
                let every = if exposure.dither_every >= 0 {
                    exposure.dither_every as u32
                } else {
                    project_dither
                };
                target.dithers.required(&exposure.filter, every)
            }
        };
        target.all[selected.plan].pre_dither = dither;
        Some(selected.plan)
    }

    fn select_exposure(&mut self, t: usize) {
        let chosen = self.choose(t);
        self.targets[t].selected = chosen;
    }

    /// Score the ready targets and return the winner: the first in load
    /// order with the highest score.
    fn score(&mut self, at: i64, previous: Option<usize>, ready: &[usize]) -> usize {
        let mut best: Option<(usize, f64)> = None;
        for &t in ready {
            let rules = self.rule_scores(at, previous, t);
            let total = rules.iter().map(|r| r.weight / 100.0 * r.raw).sum::<f64>();
            self.targets[t].score = Some(Score { total, rules });
            if best.is_none_or(|(_, top)| total > top) {
                best = Some((t, total));
            }
        }
        let winner = best.map_or(ready[0], |(t, _)| t);
        for &t in ready {
            if t != winner {
                self.targets[t].rejected = Some(TargetReason::LowerScore);
            }
        }
        winner
    }

    fn rule_scores(&self, at: i64, previous: Option<usize>, t: usize) -> Vec<RuleScore> {
        let target = &self.targets[t];
        let project = &self.projects[target.project];
        let mut rules = Vec::new();
        for (index, rule) in Rule::ALL.into_iter().enumerate() {
            let weight = project.weights[index];
            if weight <= 0.0 {
                continue;
            }
            let raw = match rule {
                Rule::MeridianFlipPenalty => match target.culmination {
                    None => 1.0,
                    Some(transit) => {
                        let flip = transit + (self.nina.minutes_after_meridian * 60_000.0) as i64;
                        if flip < at || at + project.minimum_time_ms < flip {
                            1.0
                        } else {
                            0.0
                        }
                    }
                },
                Rule::MeridianWindowPriority => {
                    if project.meridian_window_ms > 0 {
                        1.0
                    } else {
                        0.0
                    }
                }
                Rule::MosaicCompletion => {
                    if !project.mosaic || project.targets.len() < 2 {
                        0.0
                    } else {
                        let others: Vec<f64> = project
                            .targets
                            .iter()
                            .filter(|&&o| o != t)
                            .map(|&o| self.target_percent(o) / 100.0)
                            .collect();
                        let average = others.iter().sum::<f64>() / others.len() as f64;
                        (average - self.target_percent(t) / 100.0).max(0.0)
                    }
                }
                Rule::PercentComplete => self.target_percent(t) / 100.0,
                Rule::ProjectPriority => match project.priority {
                    ProjectPriority::Low => 0.0,
                    ProjectPriority::Normal => 0.5,
                    ProjectPriority::High => 1.0,
                },
                Rule::SettingSoonest => {
                    let end = target.end.unwrap_or(at);
                    let noon = self.clock.noon(end);
                    let before = if noon < end { noon } else { noon - DAY_MS };
                    1.0 - (end - before) as f64 / DAY_MS as f64
                }
                Rule::SmartExposureOrder => {
                    if project.smart {
                        target.selected.map_or(0.0, |e| target.all[e].moon_score)
                    } else {
                        0.0
                    }
                }
                Rule::TargetSwitchPenalty => {
                    if previous.is_some_and(|p| self.same_target(t, p)) {
                        1.0
                    } else {
                        0.0
                    }
                }
            };
            rules.push(RuleScore { rule, raw, weight });
        }
        rules
    }

    /// A target's percent complete: the mean over its working and completed
    /// plans.
    pub(crate) fn target_percent(&self, t: usize) -> f64 {
        let target = &self.targets[t];
        let grader = self.projects[target.project].grader;
        let all: Vec<usize> = target
            .plans
            .iter()
            .chain(&target.completed)
            .copied()
            .collect();
        if all.is_empty() {
            return 0.0;
        }
        all.iter()
            .map(|&i| target.all[i].percent_complete(grader, &self.preferences))
            .sum::<f64>()
            / all.len() as f64
    }

    /// The target to wait for, when it can start, and why it was set aside.
    fn next_possible(
        &mut self,
        at: i64,
    ) -> Result<Option<(usize, i64, Option<TargetReason>)>, NoNight> {
        let mut best: Option<(usize, i64, Option<TargetReason>)> = None;
        for p in 0..self.projects.len() {
            for t in self.projects[p].targets.clone() {
                let reason = self.targets[t].rejected;
                if reason.is_some_and(|r| !r.waitable()) {
                    continue;
                }
                {
                    let target = &mut self.targets[t];
                    target.rejected = None;
                    for plan in target.plans.clone() {
                        target.all[plan].rejected = None;
                    }
                    if target.start.is_some_and(|s| s < at) {
                        target.start = Some(at);
                    }
                }
                self.check_future(t)?;
                let target = &self.targets[t];
                if target.rejected.is_none()
                    && let Some(start) = target.start
                    && start > at
                    && best.is_none_or(|(_, top, _)| start < top)
                {
                    best = Some((t, start, reason));
                }
            }
        }
        Ok(best)
    }

    /// Walk forward from a target's start until the Moon and twilight allow
    /// one of its exposures, or it leaves the sky.
    fn check_future(&mut self, t: usize) -> Result<(), NoNight> {
        let Some(mut at) = self.targets[t].start else {
            self.targets[t].rejected = Some(TargetReason::NotVisible);
            return Ok(());
        };
        let night = self.night_at(at)?;
        let Some(seen) = self.visibility(t, &night) else {
            self.targets[t].rejected = Some(TargetReason::NotVisible);
            return Ok(());
        };
        let minimum = self.projects[self.targets[t].project].minimum_time_ms;
        at = seen.next_safe_start(at, at + minimum);
        let preferences = self.preferences.clone();
        let grader = self.projects[self.targets[t].project].grader;
        self.targets[t].blocked = None;
        for _ in 0..10_000 {
            if self.targets[t].rejected.is_none() {
                self.moon_filter_target(at, t);
                let target = &mut self.targets[t];
                if target
                    .plans
                    .iter()
                    .all(|&p| target.all[p].rejected.is_some())
                {
                    target.rejected = Some(TargetReason::MoonAvoidance);
                }
            }
            if self.targets[t].rejected.is_none() {
                let level = night.level(at);
                let target = &mut self.targets[t];
                for plan in target.plans.clone() {
                    let exposure = &mut target.all[plan];
                    if exposure.rejected.is_some() || !exposure.incomplete(grader, &preferences) {
                        continue;
                    }
                    if level.is_none_or(|level| twilight_rejects(exposure, at, &night, level)) {
                        exposure.rejected = Some(ExposureReason::Twilight);
                    }
                }
                if target
                    .plans
                    .iter()
                    .all(|&p| target.all[p].rejected.is_some())
                {
                    target.rejected = Some(TargetReason::Twilight);
                }
            }
            if self.targets[t].rejected.is_none() {
                self.targets[t].start = Some(at);
                self.targets[t].blocked = None;
                return Ok(());
            }
            self.targets[t].blocked = self.targets[t].rejected;
            at += self.sampling.future_step_ms;
            {
                let target = &mut self.targets[t];
                target.rejected = None;
                for plan in target.plans.clone() {
                    target.all[plan].rejected = None;
                }
            }
            // Visible: go on from its window's start. Set aside for now: jump
            // ahead to when it could start. Otherwise it is done tonight.
            let visible = self.visible(at, t, &night);
            let later = !visible
                && self.targets[t].rejected.is_some_and(|r| {
                    matches!(
                        r,
                        TargetReason::NotYetVisible
                            | TargetReason::BeforeMeridianWindow
                            | TargetReason::MeridianFlipClipped
                            | TargetReason::MaxAltitude
                    )
                })
                && self.targets[t].start.is_some_and(|s| s > at);
            if visible || later {
                at = self.targets[t].start.unwrap_or(at);
            } else {
                return Ok(());
            }
        }
        self.targets[t].rejected = Some(TargetReason::NotVisible);
        Ok(())
    }

    /// The exposure the last plan chose was taken: the selector, the dither
    /// count and the frame counts move on.
    pub(crate) fn taken(&mut self, t: usize, exposure: usize, acceptance: f64) {
        let grader = self.projects[self.targets[t].project].grader;
        let target = &mut self.targets[t];
        target.selector.taken(exposure);
        let dithered = target.all[exposure].pre_dither;
        let filter = target.all[exposure].filter.clone();
        target.dithers.taken(&filter, dithered);
        let plan = &mut target.all[exposure];
        plan.acquired += 1.0;
        if grader {
            plan.accepted += acceptance;
        }
    }

    /// Forget a target's dither count and smart rotation, as the preview
    /// does when it returns to one.
    pub(crate) fn reset_target(&mut self, t: usize) {
        self.targets[t].dithers.clear();
        self.targets[t].selector.reset();
    }

    /// Start a new night: the dither counts and rotations Target Scheduler
    /// keeps for a session are gone, and every target is planned afresh.
    pub(crate) fn new_night(&mut self) {
        for target in &mut self.targets {
            target.dithers.clear();
            target.selector.reset();
            target.start = None;
            target.end = None;
            target.selected = None;
        }
        self.clear_rejections();
    }

    /// Whether a project still owes frames.
    pub(crate) fn project_complete(&self, p: usize) -> bool {
        let grader = self.projects[p].grader;
        self.projects[p].targets.iter().all(|&t| {
            let target = &self.targets[t];
            target
                .plans
                .iter()
                .all(|&i| target.all[i].remaining(grader, &self.preferences) <= 1e-9)
        })
    }

    /// Whether the twilight at `at` allows any owed exposure.
    pub(crate) fn twilight_allows_any(&mut self, at: i64) -> bool {
        let Ok(night) = self.night_at(at) else {
            return false;
        };
        let Some(level) = night.level(at) else {
            return false;
        };
        let preferences = self.preferences.clone();
        self.targets.iter().any(|target| {
            let grader = self.projects[target.project].grader;
            target.plans.iter().any(|&i| {
                let exposure = &target.all[i];
                exposure.remaining(grader, &preferences) > 1e-9
                    && !twilight_rejects(exposure, at, &night, level)
            })
        })
    }

    /// The first minute from `from`, before `to`, when twilight allows an
    /// owed exposure.
    pub(crate) fn twilight_opens(&mut self, from: i64, to: i64) -> Option<i64> {
        let mut at = from;
        while at < to {
            if self.twilight_allows_any(at) {
                return Some(at);
            }
            at += 60_000;
        }
        None
    }

    /// Whether every owed exposure's dark time ends within its project's
    /// minimum time from `at`.
    fn darkness_closing(&mut self, at: i64) -> bool {
        let Ok(night) = self.night_at(at) else {
            return true;
        };
        let preferences = self.preferences.clone();
        !self.targets.iter().any(|target| {
            let project = &self.projects[target.project];
            target.plans.iter().any(|&i| {
                let exposure = &target.all[i];
                exposure.remaining(project.grader, &preferences) > 1e-9
                    && night.span(exposure.twilight_level).is_some_and(|(_, end)| {
                        end - exposure.twilight_offset_ms.max(0) - at >= project.minimum_time_ms
                    })
            })
        })
    }

    /// Why the planner has nothing more tonight, by what set each owed
    /// target aside.
    pub(crate) fn done_reason(&mut self, at: i64) -> DoneReason {
        if (0..self.projects.len()).all(|p| self.project_complete(p)) {
            return DoneReason::Complete;
        }
        let mut found: Option<DoneReason> = None;
        for t in 0..self.targets.len() {
            let project = self.targets[t].project;
            let grader = self.projects[project].grader;
            let windowed = self.projects[project].meridian_window_ms > 0;
            let preferences = self.preferences.clone();
            let target = &self.targets[t];
            if target
                .plans
                .iter()
                .all(|&i| target.all[i].remaining(grader, &preferences) <= 1e-9)
            {
                continue;
            }
            let mut reason = target.blocked.or(target.rejected);
            // Up, but left with too little of its meridian window.
            if windowed && reason == Some(TargetReason::NotVisible) && self.above_horizon(t, at) {
                reason = Some(TargetReason::MeridianWindowClipped);
            }
            let reason = match reason {
                Some(TargetReason::MoonAvoidance) => DoneReason::Moon,
                Some(TargetReason::Twilight) => DoneReason::Twilight,
                Some(
                    TargetReason::MeridianWindowClipped
                    | TargetReason::BeforeMeridianWindow
                    | TargetReason::MeridianFlipClipped,
                ) => DoneReason::Meridian,
                Some(
                    TargetReason::NotVisible
                    | TargetReason::NeverRises
                    | TargetReason::MaxAltitude
                    | TargetReason::NotYetVisible,
                ) => DoneReason::BelowLimits,
                _ => DoneReason::NothingEligible,
            };
            found = match found {
                None => Some(reason),
                Some(previous) if previous == reason => Some(reason),
                Some(_) => Some(DoneReason::NothingEligible),
            };
        }
        let found = found.unwrap_or(DoneReason::Complete);
        if matches!(found, DoneReason::BelowLimits | DoneReason::NothingEligible)
            && self.darkness_closing(at)
        {
            return DoneReason::Twilight;
        }
        found
    }

    /// Whether a target is above its horizon at an instant.
    pub(crate) fn above_horizon(&mut self, t: usize, at: i64) -> bool {
        match self.night_at(at) {
            Ok(night) => self
                .visibility(t, &night)
                .is_some_and(|seen| seen.visible_at(at)),
            Err(_) => false,
        }
    }
}

impl PlanningProject {
    pub(crate) fn rejected_any(&self) -> bool {
        self.rejected.is_some()
    }
}

fn planning_project(project: &Project) -> PlanningProject {
    let mut weights = [0.0; 8];
    for (index, rule) in Rule::ALL.into_iter().enumerate() {
        weights[index] = project.rule_weight(rule.name(), rule.default_weight());
    }
    PlanningProject {
        id: project.id,
        name: project.name.clone(),
        priority: project.priority,
        minimum_time_ms: i64::from(project.minimum_time_minutes) * 60_000,
        minimum_altitude: project.minimum_altitude_degrees,
        maximum_altitude: project.maximum_altitude_degrees,
        use_custom_horizon: project.use_custom_horizon,
        horizon_offset: project.horizon_offset_degrees,
        meridian_window_ms: i64::from(project.meridian_window_minutes) * 60_000,
        dither_every: project.dither_every,
        grader: project.enable_grader,
        mosaic: project.is_mosaic,
        smart: project.smart_exposure_order,
        weights,
        targets: Vec::new(),
        rejected: None,
    }
}

/// Split a target's active plans into the working list and the completed
/// list, rejecting the completed as complete.
fn split(target: &mut Target, active: &[usize], grader: bool, preferences: &Preferences) {
    target.plans.clear();
    target.completed.clear();
    for &plan in active {
        if target.all[plan].incomplete(grader, preferences) {
            target.plans.push(plan);
        } else {
            target.all[plan].rejected = Some(ExposureReason::Complete);
            target.completed.push(plan);
        }
    }
}

/// Target Scheduler's twilight test of one exposure at an instant.
fn twilight_rejects(
    exposure: &Exposure,
    at: i64,
    night: &Twilight,
    current: TwilightLevel,
) -> bool {
    let offset = exposure.twilight_offset_ms;
    if offset == 0 {
        return current > exposure.twilight_level;
    }
    if offset > 0 {
        return match night.span(exposure.twilight_level) {
            Some((start, end)) => !(start + offset <= at && at <= end - offset),
            // Target Scheduler throws; nothing at this level tonight.
            None => true,
        };
    }
    false
}

/// Target Scheduler's Moon avoidance test of one exposure at an instant,
/// which also sets the exposure's Moon score.
fn moon_rejects(
    frame: &Frame,
    moon: &mut MoonAt,
    exposure: &mut Exposure,
    at: i64,
    ra_hours: f64,
    dec_degrees: f64,
) -> bool {
    let policy = exposure.moon.clone();
    if !policy.enabled {
        exposure.moon_score = 0.0;
        return false;
    }
    exposure.moon_score = exposure.score_moon();
    let (seen, altitude) = moon.seen(frame);
    let mut separation = policy.separation_degrees;
    let mut width = policy.width_days;
    let relaxing = policy.relax_degrees_per_degree > 0.0;
    if altitude <= policy.relax_max_altitude_degrees && relaxing {
        separation +=
            policy.relax_degrees_per_degree * (altitude - policy.relax_max_altitude_degrees);
        width *= (altitude - policy.relax_min_altitude_degrees)
            / (policy.relax_max_altitude_degrees - policy.relax_min_altitude_degrees);
    }
    if altitude <= policy.relax_min_altitude_degrees && relaxing {
        return false;
    }
    if altitude >= policy.relax_max_altitude_degrees && policy.moon_down {
        return true;
    }
    if separation <= 0.0 {
        return false;
    }
    let Some(age) = *moon.age_days.get_or_insert_with(|| frame.moon_age_days(at)) else {
        return false;
    };
    const CYCLE: f64 = 29.53059;
    let need = if width == 0.0 {
        0.0
    } else {
        separation / (1.0 + ((0.5 - age / CYCLE) / (width / CYCLE)).powi(2))
    };
    frame.separation_from(seen, ra_hours, dec_degrees) < need
}

#[cfg(test)]
mod tests {
    use super::*;
    use psf_guard_director_core::visibility::Site;

    /// The Moon kept for an instant is the Moon each test computed for
    /// itself before: its altitude, age and every target's separation, to
    /// the bit.
    #[test]
    fn the_moon_kept_for_an_instant_is_the_moon_each_test_computed() {
        let site = Site {
            latitude_degrees: 34.2,
            longitude_degrees: -118.3,
            elevation_meters: 400.0,
        };
        // Noon in Los Angeles on 2026-09-26, the day of the full Moon.
        let noon = 1_790_449_200_000;
        let frame = Frame::new(site, noon);
        for step in 0..48 {
            let at = noon + step * 30 * 60_000;
            let mut moon = MoonAt::new(at);
            let (seen, altitude) = moon.seen(&frame);
            assert_eq!(altitude.to_bits(), frame.moon_altitude(at).to_bits());
            for (ra_hours, dec_degrees) in [(2.55, 61.45), (12.0, -20.0), (20.7, 44.3)] {
                assert_eq!(
                    frame.separation_from(seen, ra_hours, dec_degrees).to_bits(),
                    frame.moon_separation(at, ra_hours, dec_degrees).to_bits()
                );
            }
            // Kept, not computed again.
            assert_eq!(moon.seen(&frame).1.to_bits(), altitude.to_bits());
        }
    }
}
