//! Read-only restart review. Passing this review is permission to request fresh
//! authority, never permission to replay an allocation or move equipment.

use super::{Motion, Phase, Snapshot};
use crate::Safety;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    /// No acquisition was ever admitted in the interrupted owner.
    Unused,
    /// All durable captures, preparation and user hooks have known outcomes.
    Settled,
    Unresolved,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    pub rig_id: String,
    pub configuration_id: String,
    pub night_id: String,
    pub now_ms: u64,
    pub operator_requested: bool,
    pub boundary: Boundary,
    pub camera_idle: bool,
    pub mount_stopped: bool,
    pub guider_stopped: bool,
    pub safety: Safety,
    pub motion: Motion,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Advice {
    OperatorReviewRequired,
    WrongScope,
    ClockReversed,
    NightEnded,
    HoldExpired,
    TerminalStop,
    UncertainRecovery,
    UnsettledExecution,
    EquipmentNotQuiescent,
    WaitForSafety,
    /// Must retain this night's identity, frozen policy, deadline and budgets.
    /// A consumed launch token and persisted probe never become reusable.
    RequestFreshAuthority,
}

pub fn review(snapshot: &Snapshot, input: &Review) -> Result<Advice, super::Error> {
    snapshot.validate()?;
    if input.rig_id != snapshot.identity.rig_id
        || input.configuration_id != snapshot.identity.configuration_id
        || input.night_id != snapshot.identity.night_id
    {
        return Ok(Advice::WrongScope);
    }
    if input.now_ms < snapshot.last_event_ms {
        return Ok(Advice::ClockReversed);
    }
    if input.now_ms >= snapshot.identity.ends_at_ms {
        return Ok(Advice::NightEnded);
    }
    if matches!(
        snapshot.phase,
        Phase::Stopping { .. } | Phase::Stopped { .. }
    ) {
        return Ok(Advice::TerminalStop);
    }
    if matches!(snapshot.phase, Phase::Recovering { .. }) {
        return Ok(Advice::UncertainRecovery);
    }
    let elapsed = input.now_ms - snapshot.last_event_ms;
    if input.now_ms >= snapshot.policy.latest_resume_ms
        || matches!(&snapshot.phase, Phase::Holding { hold } if input.now_ms >= hold.expires_at_ms
            || snapshot.total_hold_ms.saturating_add(elapsed) >= snapshot.policy.maximum_hold_ms)
        || matches!(&snapshot.phase, Phase::WeatherHolding { .. } if snapshot.policy.weather.as_ref()
            .is_none_or(|p| snapshot.weather_hold_ms.saturating_add(elapsed) >= p.maximum_hold_ms))
    {
        return Ok(Advice::HoldExpired);
    }
    if !input.operator_requested {
        return Ok(Advice::OperatorReviewRequired);
    }
    if matches!(input.boundary, Boundary::Unknown | Boundary::Unresolved) {
        return Ok(Advice::UnsettledExecution);
    }
    if !input.camera_idle || !input.mount_stopped || !input.guider_stopped {
        return Ok(Advice::EquipmentNotQuiescent);
    }
    if input.safety != Safety::Safe || input.motion != Motion::Permitted {
        return Ok(Advice::WaitForSafety);
    }
    Ok(Advice::RequestFreshAuthority)
}
