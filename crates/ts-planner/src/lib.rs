// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! An emulation of N.I.N.A. Target Scheduler's planner: from the rows a rig's
//! database holds for one N.I.N.A. profile, what Target Scheduler would shoot
//! over a night. A prediction for PSF Guard's night plans; the plugin still
//! decides on the rig.
//!
//! Astronomy comes from the shared Director core. Target Scheduler's rules
//! stay here, out of the core.

mod astro;
mod planner;
mod select;
mod simulate;
pub mod snapshot;
mod twilight;
mod visibility;

pub use planner::{NinaProfile, Rule, RuleScore, Score, TargetReason};
pub use simulate::{
    simulate, Block, Candidate, Decision, DecisionKind, Estimate, FilterRun, Gap, GapReason, Mode,
    NightSchedule, Overheads, PlanProgress, Request, Simulation, TargetRef,
};
pub use snapshot::Snapshot;
pub use twilight::Clock;

/// The Target Scheduler release whose planning this crate follows.
pub const TARGET_SCHEDULER_VERSION: &str = "5";
/// The Target Scheduler database schema version it reads.
pub const SCHEMA_VERSION: u32 = 23;
