// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What Target Scheduler plans from: one N.I.N.A. profile's rows in a rig's
//! database, as the planner reads them. The host reads the database; this
//! crate never opens it.

use psf_guard_director_core::{moon::MoonPolicy, visibility::IcrsPosition};
use serde::{Deserialize, Serialize};

/// One N.I.N.A. profile's planning input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// The N.I.N.A. profile the rows belong to.
    pub profile_id: String,
    pub preferences: Preferences,
    pub templates: Vec<ExposureTemplate>,
    pub projects: Vec<Project>,
}

/// The profile preferences that change planning, with Target Scheduler's
/// defaults for a profile that has no row. (`enableSmartPlanWindow` is left
/// out: Target Scheduler 5 never reads it.)
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    /// Centre (and rotate) on a target change. Off, a target change does
    /// nothing at all, not even a slew.
    pub enable_slew_center: bool,
    /// Park during waits over a minute.
    pub park_on_wait: bool,
    /// With the grader off, a plan is done once it has acquired this
    /// percentage of its desired frames.
    pub exposure_throttle_percent: f64,
    /// With the grader on, a plan's percent complete counts acquired frames
    /// until it has acquired this percentage of its desired frames, and
    /// accepted frames after that. 0 counts accepted frames throughout.
    pub delay_grading_percent: f64,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enable_slew_center: true,
            park_on_wait: false,
            exposure_throttle_percent: 125.0,
            delay_grading_percent: 80.0,
        }
    }
}

/// `project.state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectState {
    Draft,
    Active,
    Inactive,
    Closed,
}

impl ProjectState {
    /// The column's value; an unknown value counts as a draft, which the
    /// planner skips.
    pub fn from_column(value: i64) -> Self {
        match value {
            1 => Self::Active,
            2 => Self::Inactive,
            3 => Self::Closed,
            _ => Self::Draft,
        }
    }
}

/// `project.priority`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectPriority {
    Low,
    Normal,
    High,
}

impl ProjectPriority {
    pub fn from_column(value: i64) -> Self {
        match value {
            0 => Self::Low,
            2 => Self::High,
            _ => Self::Normal,
        }
    }
}

/// `exposuretemplate.twilightlevel`: the brightest sky a template accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TwilightLevel {
    /// Sun below −18°.
    Nighttime,
    /// Sun below −12°.
    Astronomical,
    /// Sun below −6°.
    Nautical,
    /// Sun below 0°.
    Civil,
}

impl TwilightLevel {
    pub fn from_column(value: i64) -> Self {
        match value {
            1 => Self::Astronomical,
            2 => Self::Nautical,
            3 => Self::Civil,
            _ => Self::Nighttime,
        }
    }

    /// The Sun altitude the level allows imaging below.
    pub fn sun_below_degrees(self) -> f64 {
        match self {
            Self::Nighttime => -18.0,
            Self::Astronomical => -12.0,
            Self::Nautical => -6.0,
            Self::Civil => 0.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposureTemplate {
    pub id: i64,
    pub name: String,
    pub filter_name: String,
    pub gain: Option<i64>,
    pub offset: Option<i64>,
    pub binning: Option<i64>,
    pub readout_mode: Option<i64>,
    pub twilight_level: TwilightLevel,
    /// Minutes the twilight limit moves: later at dusk and earlier at dawn
    /// when positive.
    pub twilight_offset_minutes: i64,
    /// Target Scheduler's Moon avoidance, in the shared core's terms.
    pub moon: MoonPolicy,
    pub maximum_humidity: f64,
    pub default_exposure_seconds: f64,
    /// −1 takes the project's dither setting.
    pub dither_every: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub id: i64,
    pub guid: Option<String>,
    pub name: String,
    pub state: ProjectState,
    pub priority: ProjectPriority,
    pub minimum_time_minutes: u32,
    pub minimum_altitude_degrees: f64,
    /// 0 is no maximum.
    pub maximum_altitude_degrees: f64,
    pub use_custom_horizon: bool,
    pub horizon_offset_degrees: f64,
    /// Minutes either side of the upper transit; 0 is off.
    pub meridian_window_minutes: u32,
    /// Exposures of one filter before switching; 0 repeats until done.
    pub filter_switch_frequency: u32,
    /// Exposures between dithers; 0 is off.
    pub dither_every: u32,
    pub enable_grader: bool,
    pub is_mosaic: bool,
    pub smart_exposure_order: bool,
    /// `ruleweight` rows by rule name. A rule with no row has its default.
    pub rule_weights: Vec<RuleWeight>,
    pub targets: Vec<Target>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleWeight {
    pub name: String,
    pub weight: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: i64,
    pub guid: Option<String>,
    pub name: String,
    pub active: bool,
    /// Target Scheduler stores right ascension in hours; this is degrees.
    pub position: IcrsPosition,
    pub rotation_degrees: f64,
    pub roi_percent: f64,
    /// In the order Target Scheduler lists them, which the override order
    /// and the filter cadence refer to by index.
    pub exposure_plans: Vec<ExposurePlan>,
    pub override_order: Vec<OrderItem>,
    pub filter_cadence: Vec<CadenceItem>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExposurePlan {
    pub id: i64,
    pub guid: Option<String>,
    pub template_id: i64,
    /// As stored: a negative value takes the template's default exposure.
    pub exposure_seconds: f64,
    pub desired: u32,
    pub acquired: u32,
    pub accepted: u32,
    pub enabled: bool,
}

/// One row of a target's override exposure order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderItem {
    pub order: i64,
    /// Target Scheduler's action code: an exposure or a dither.
    pub action: i64,
    /// For an exposure, the index of the plan in `exposure_plans`.
    pub reference_index: Option<i64>,
}

/// One row of a target's filter cadence, which Target Scheduler advances
/// and saves as it plans.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CadenceItem {
    pub order: i64,
    /// The item the cadence takes next.
    pub next: bool,
    pub action: i64,
    pub reference_index: Option<i64>,
}

impl Project {
    /// This project's weight for a rule: its `ruleweight` row, else the
    /// rule's default.
    pub fn rule_weight(&self, name: &str, default: f64) -> f64 {
        self.rule_weights
            .iter()
            .find(|weight| weight.name == name)
            .map_or(default, |weight| weight.weight)
    }
}

impl ExposurePlan {
    /// Frames the plan still owes, as Target Scheduler counts them: with the
    /// grader on, the desired frames less those accepted; with it off, the
    /// throttle's share of the desired frames (truncated) less those
    /// acquired.
    pub fn remaining(&self, enable_grader: bool, throttle_percent: f64) -> u32 {
        if enable_grader {
            self.desired.saturating_sub(self.accepted)
        } else {
            throttle_at(self.desired, throttle_percent).saturating_sub(self.acquired)
        }
    }

    /// The plan's exposure, in seconds.
    pub fn exposure_seconds(&self, template: &ExposureTemplate) -> f64 {
        if self.exposure_seconds < 0.0 {
            template.default_exposure_seconds
        } else {
            self.exposure_seconds
        }
    }
}

/// `(int)(throttle × desired)`: the acquisitions at which a plan without
/// grading is done.
pub fn throttle_at(desired: u32, throttle_percent: f64) -> u32 {
    let at = throttle_percent / 100.0 * f64::from(desired);
    if at.is_finite() && at > 0.0 {
        at.min(f64::from(u32::MAX)) as u32
    } else {
        0
    }
}
