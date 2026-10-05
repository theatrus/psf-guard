//! Session recovery policy, independent of allocations and hardware adapters.
//! Decisions describe required work, never grant permission to move equipment.
//! Quality verdicts require an upstream evidence classifier; grades alone are
//! not quality observations. Terminal stops cannot be cleared by a resume event.

use crate::Safety;
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;
const DAY_MS: u64 = 86_400_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub rig_id: String,
    pub configuration_id: String,
    /// An explicitly admitted observing night, not a date or allocation ID.
    pub night_id: String,
    pub starts_at_ms: u64,
    pub ends_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QualityMode {
    Disabled,
    MonitorOnly,
    Pause,
    ParkAndStop,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub revision: u64,
    pub quality_mode: QualityMode,
    pub bad_samples: u32,
    pub good_probes: u32,
    pub cooldown_ms: u64,
    pub maximum_hold_ms: u64,
    pub maximum_probes: u32,
    pub operation_timeout_ms: u64,
    pub evidence_max_age_ms: u64,
    pub latest_resume_ms: u64,
    pub maximum_consecutive_failures: u32,
    pub maximum_total_failures: u32,
    /// A commissioned preference, still subordinate to current roof clearance.
    pub park_on_stop: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weather: Option<WeatherPolicy>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WeatherPolicy {
    pub stable_safe_ms: u64,
    pub maximum_hold_ms: u64,
    pub maximum_interruptions: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Motion {
    Permitted,
    Prohibited,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Conditions {
    pub safety: Safety,
    pub motion: Motion,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Guide,
    Focus,
    Center,
    Slew,
    Capture,
    Other,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub attempt_id: String,
    pub operation: Operation,
    pub device_id: String,
    pub target_id: String,
    /// Unknown completion can never be retried by this policy.
    pub uncertain: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QualityContext {
    pub target_id: String,
    pub filter_id: String,
    pub exposure_ms: u64,
    pub bin_x: u16,
    pub bin_y: u16,
    /// Frozen known-good reference. Rolling baselines cannot imply recovery.
    pub reference_id: String,
    pub source: String,
    pub algorithm_revision: String,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    Unknown,
    CorroboratedPoor,
    ConfirmedGood,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct QualitySample {
    pub rig_id: String,
    pub configuration_id: String,
    pub capture_id: String,
    pub observed_at_ms: u64,
    pub context: QualityContext,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Cause {
    Quality { context: QualityContext },
    Equipment { failure: Failure },
    Safety {},
    Enclosure {},
    Operator {},
    NightEnded {},
    HoldExpired {},
    ProbeBudget {},
    RecoveryUncertain {},
    FailureBudget {},
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hold {
    pub cause: Cause,
    pub started_at_ms: u64,
    pub retry_at_ms: u64,
    pub expires_at_ms: u64,
    pub probes: u32,
    pub good_probes: u32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Shutdown {
    NoParkRequested,
    MotionBlocked,
    Parked,
    ParkFailed,
    ParkUncertain,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Phase {
    Acquiring {},
    WeatherHolding {
        cause: Cause,
        started_at_ms: u64,
        stable_since_ms: Option<u64>,
    },
    Holding {
        hold: Hold,
    },
    Recovering {
        hold: Hold,
        attempt_id: String,
        started_at_ms: u64,
        deadline_ms: u64,
    },
    Stopping {
        cause: Cause,
        deadline_ms: u64,
        park_attempt_id: Option<String>,
    },
    Stopped {
        cause: Cause,
        shutdown: Shutdown,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryResult {
    Quality { sample: Box<QualitySample> },
    EquipmentVerified {},
    Failed {},
    Uncertain {},
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParkResult {
    Parked,
    Failed,
    Uncertain,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    Tick {},
    WeatherInterrupted {
        enclosure: bool,
    },
    ResumeWeather {},
    Quality {
        sample: QualitySample,
    },
    Failure {
        failure: Failure,
    },
    BeginRecovery {
        attempt_id: String,
    },
    RecoveryCompleted {
        attempt_id: String,
        result: RecoveryResult,
    },
    StopNight {},
    BeginPark {
        attempt_id: String,
    },
    ParkCompleted {
        attempt_id: String,
        result: ParkResult,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FailureCount {
    pub operation: Operation,
    pub device_id: String,
    pub consecutive: u32,
    pub total: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub identity: Identity,
    pub policy: Policy,
    pub last_event_ms: u64,
    pub phase: Phase,
    pub consecutive_bad: u32,
    pub quality_context: Option<QualityContext>,
    pub last_quality_ms: Option<u64>,
    pub failures: Vec<FailureCount>,
    pub total_failures: u32,
    pub total_hold_ms: u64,
    pub probes_spent: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub weather_interruptions: u32,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub weather_hold_ms: u64,
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}
fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput,
    WrongPhase,
    StaleEvidence,
    ChangedReference,
    ClockReversed,
    LimitReached,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidInput => "Invalid recovery policy, identity or state",
            Self::WrongPhase => "Recovery event does not match the current phase or attempt",
            Self::StaleEvidence => "Recovery evidence is stale, out of order or from the future",
            Self::ChangedReference => "Recovery evidence changed its comparison context",
            Self::ClockReversed => "Recovery clock moved backwards",
            Self::LimitReached => "Recovery accounting limit reached",
        })
    }
}
impl std::error::Error for Error {}

fn id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_graphic())
}

impl Identity {
    pub fn validate(&self) -> Result<(), Error> {
        if !id(&self.rig_id)
            || !id(&self.configuration_id)
            || !id(&self.night_id)
            || self.starts_at_ms >= self.ends_at_ms
            || self.ends_at_ms > i64::MAX as u64
            || self.ends_at_ms - self.starts_at_ms > 2 * DAY_MS
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

impl Policy {
    pub fn validate(&self, identity: &Identity) -> Result<(), Error> {
        if self.weather.as_ref().is_some_and(|w| {
            w.stable_safe_ms == 0
                || w.stable_safe_ms >= w.maximum_hold_ms
                || w.maximum_hold_ms > DAY_MS
                || !(1..=100).contains(&w.maximum_interruptions)
        }) {
            return Err(Error::InvalidInput);
        }
        if self.revision == 0
            || (self.quality_mode == QualityMode::ParkAndStop && !self.park_on_stop)
            || !(1..=100).contains(&self.bad_samples)
            || !(1..=100).contains(&self.good_probes)
            || !(1..=100).contains(&self.maximum_probes)
            || self.good_probes > self.maximum_probes
            || self.cooldown_ms == 0
            || self.cooldown_ms > self.maximum_hold_ms
            || self.maximum_hold_ms > DAY_MS
            || self.operation_timeout_ms == 0
            || self.operation_timeout_ms > DAY_MS
            || self.evidence_max_age_ms == 0
            || self.evidence_max_age_ms > DAY_MS
            || self.latest_resume_ms <= identity.starts_at_ms
            || self.latest_resume_ms > identity.ends_at_ms
            || !(1..=100).contains(&self.maximum_consecutive_failures)
            || !(1..=10000).contains(&self.maximum_total_failures)
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

impl QualityContext {
    fn validate(&self) -> Result<(), Error> {
        if [
            &self.target_id,
            &self.filter_id,
            &self.reference_id,
            &self.source,
            &self.algorithm_revision,
        ]
        .into_iter()
        .any(|s| !id(s))
            || self.exposure_ms == 0
            || self.exposure_ms > DAY_MS
            || !(1..=16).contains(&self.bin_x)
            || !(1..=16).contains(&self.bin_y)
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

impl Failure {
    fn validate(&self) -> Result<(), Error> {
        if !id(&self.attempt_id) || !id(&self.device_id) || !id(&self.target_id) {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }
}

impl Cause {
    fn validate(&self) -> Result<(), Error> {
        match self {
            Self::Quality { context } => context.validate(),
            Self::Equipment { failure } => failure.validate(),
            _ => Ok(()),
        }
    }
}

impl Snapshot {
    /// Validate persisted input before using it to make a decision. This is an
    /// integrity check, not authentication or authority to operate equipment.
    pub fn validate(&self) -> Result<(), Error> {
        self.identity.validate()?;
        self.policy.validate(&self.identity)?;
        if self.policy.weather.as_ref().map_or(
            self.weather_interruptions != 0 || self.weather_hold_ms != 0,
            |weather| self.weather_interruptions > weather.maximum_interruptions,
        ) {
            return Err(Error::InvalidInput);
        }
        if self.schema_version != VERSION
            || self.last_event_ms < self.identity.starts_at_ms
            || self.last_event_ms > i64::MAX as u64
            || self.probes_spent > self.policy.maximum_probes
            || self.failures.len() > 64
            || self
                .last_quality_ms
                .is_some_and(|t| t < self.identity.starts_at_ms || t > self.last_event_ms)
            || self.total_failures > self.policy.maximum_total_failures
        {
            return Err(Error::InvalidInput);
        }
        if let Some(context) = &self.quality_context {
            context.validate()?;
        }
        let mut total = 0u64;
        for (index, count) in self.failures.iter().enumerate() {
            if !id(&count.device_id)
                || count.total == 0
                || count.consecutive > count.total
                || self.failures[..index].iter().any(|previous| {
                    previous.operation == count.operation && previous.device_id == count.device_id
                })
            {
                return Err(Error::InvalidInput);
            }
            total += u64::from(count.total);
        }
        if total != u64::from(self.total_failures) {
            return Err(Error::InvalidInput);
        }
        match &self.phase {
            Phase::WeatherHolding {
                cause,
                started_at_ms,
                stable_since_ms,
            } => {
                let weather = self.policy.weather.as_ref().ok_or(Error::InvalidInput)?;
                if !matches!(cause, Cause::Safety {} | Cause::Enclosure {})
                    || *started_at_ms < self.identity.starts_at_ms
                    || *started_at_ms > self.last_event_ms
                    || stable_since_ms.is_some_and(|t| t < *started_at_ms || t > self.last_event_ms)
                    || self.weather_interruptions == 0
                    || self.weather_interruptions > weather.maximum_interruptions
                    || self.weather_hold_ms >= weather.maximum_hold_ms
                    || self.last_event_ms >= self.identity.ends_at_ms
                {
                    return Err(Error::InvalidInput);
                }
            }
            Phase::Holding { hold } | Phase::Recovering { hold, .. } => {
                hold.cause.validate()?;
                if !matches!(hold.cause, Cause::Quality { .. } | Cause::Equipment { .. })
                    || hold.started_at_ms < self.identity.starts_at_ms
                    || hold.started_at_ms > self.last_event_ms
                    || hold.retry_at_ms < hold.started_at_ms
                    || hold.expires_at_ms <= self.last_event_ms
                    || hold.expires_at_ms > self.policy.latest_resume_ms
                    || hold.probes > self.probes_spent
                    || hold.good_probes > hold.probes
                    || self.total_hold_ms >= self.policy.maximum_hold_ms
                {
                    return Err(Error::InvalidInput);
                }
                if let Phase::Recovering {
                    attempt_id,
                    started_at_ms,
                    deadline_ms,
                    ..
                } = &self.phase
                    && (!id(attempt_id)
                        || hold.probes == 0
                        || *started_at_ms < hold.retry_at_ms
                        || *started_at_ms > self.last_event_ms
                        || *deadline_ms <= self.last_event_ms
                        || *deadline_ms > hold.expires_at_ms)
                {
                    return Err(Error::InvalidInput);
                }
            }
            Phase::Stopping {
                cause,
                deadline_ms,
                park_attempt_id,
            } => {
                cause.validate()?;
                if !self.policy.park_on_stop
                    || *deadline_ms <= self.last_event_ms
                    || park_attempt_id.as_ref().is_some_and(|attempt| !id(attempt))
                {
                    return Err(Error::InvalidInput);
                }
            }
            Phase::Stopped { cause, .. } => cause.validate()?,
            Phase::Acquiring {} => {
                if self.last_event_ms >= self.identity.ends_at_ms {
                    return Err(Error::InvalidInput);
                }
            }
        }
        Ok(())
    }

    pub fn new(identity: Identity, policy: Policy, now_ms: u64) -> Result<Self, Error> {
        identity.validate()?;
        policy.validate(&identity)?;
        if now_ms < identity.starts_at_ms || now_ms >= identity.ends_at_ms {
            return Err(Error::InvalidInput);
        }
        Ok(Self {
            schema_version: VERSION,
            identity,
            policy,
            last_event_ms: now_ms,
            phase: Phase::Acquiring {},
            consecutive_bad: 0,
            quality_context: None,
            last_quality_ms: None,
            failures: vec![],
            total_failures: 0,
            total_hold_ms: 0,
            probes_spent: 0,
            weather_interruptions: 0,
            weather_hold_ms: 0,
        })
    }

    /// Pure transition. On error the original snapshot is unchanged. The host
    /// must stop dispatch on errors and persist a transition before acting on it.
    pub fn apply(&self, now: u64, conditions: Conditions, event: &Event) -> Result<Self, Error> {
        self.validate()?;
        if now > i64::MAX as u64 {
            return Err(Error::InvalidInput);
        }
        if now < self.last_event_ms {
            return Err(Error::ClockReversed);
        }
        let mut next = self.clone();
        next.last_event_ms = now;
        if matches!(self.phase, Phase::Holding { .. } | Phase::Recovering { .. }) {
            next.total_hold_ms = next.total_hold_ms.saturating_add(now - self.last_event_ms);
        }
        if matches!(self.phase, Phase::Stopped { .. }) {
            return Ok(next);
        }
        if matches!(self.phase, Phase::Stopping { .. }) {
            next.shutdown(now, conditions.motion, event)?;
            return Ok(next);
        }
        if self.policy.weather.is_some() {
            if matches!(self.phase, Phase::WeatherHolding { .. }) {
                next.weather_hold_ms = next
                    .weather_hold_ms
                    .saturating_add(now - self.last_event_ms);
            }
            // Operator and night-end stops remain terminal, even during bad weather.
            if matches!(event, Event::StopNight {}) {
                next.stop(Cause::Operator {}, now, conditions.motion);
                return Ok(next);
            }
            if now >= self.identity.ends_at_ms {
                next.stop(Cause::NightEnded {}, now, conditions.motion);
                return Ok(next);
            }
            if self.weather_transition(&mut next, now, conditions, event)? {
                return Ok(next);
            }
        }
        // Safety and a closing roof preempt recovery, including operator stop.
        if conditions.safety != Safety::Safe {
            next.stop(Cause::Safety {}, now, conditions.motion);
            return Ok(next);
        }
        if conditions.motion == Motion::Prohibited
            || (conditions.motion == Motion::Unknown
                && matches!(self.phase, Phase::Recovering { .. }))
        {
            next.stop(Cause::Enclosure {}, now, conditions.motion);
            return Ok(next);
        }
        if matches!(event, Event::StopNight {}) {
            next.stop(Cause::Operator {}, now, conditions.motion);
            return Ok(next);
        }
        if now >= self.identity.ends_at_ms {
            next.stop(Cause::NightEnded {}, now, conditions.motion);
            return Ok(next);
        }
        match &self.phase {
            Phase::Holding { hold } | Phase::Recovering { hold, .. }
                if now >= hold.expires_at_ms =>
            {
                next.stop(Cause::HoldExpired {}, now, conditions.motion);
                return Ok(next);
            }
            Phase::Recovering { deadline_ms, .. } if now >= *deadline_ms => {
                next.stop(Cause::RecoveryUncertain {}, now, conditions.motion);
                return Ok(next);
            }
            _ => {}
        }
        match event {
            Event::WeatherInterrupted { .. } | Event::ResumeWeather {} => {
                return Err(Error::WrongPhase)
            }
            Event::Tick {} => {}
            Event::Quality { sample } => {
                if !matches!(self.phase, Phase::Acquiring {}) {
                    return Err(Error::WrongPhase);
                }
                next.sample(sample, now)?;
                if self.policy.quality_mode != QualityMode::Disabled {
                    if self.quality_context.as_ref() != Some(&sample.context) {
                        next.consecutive_bad = 0;
                    }
                    next.quality_context = Some(sample.context.clone());
                    next.consecutive_bad = if sample.verdict == Verdict::CorroboratedPoor {
                        next.consecutive_bad.saturating_add(1)
                    } else {
                        0
                    };
                    if next.consecutive_bad >= self.policy.bad_samples {
                        let cause = Cause::Quality {
                            context: sample.context.clone(),
                        };
                        match self.policy.quality_mode {
                            QualityMode::Pause => next.hold(cause, now, conditions.motion),
                            QualityMode::ParkAndStop => next.stop(cause, now, conditions.motion),
                            _ => {}
                        }
                    }
                }
            }
            Event::Failure { failure } => {
                failure.validate()?;
                let at = next.failures.iter().position(|c| {
                    c.operation == failure.operation && c.device_id == failure.device_id
                });
                let at = if let Some(at) = at {
                    at
                } else {
                    if next.failures.len() >= 64 {
                        return Err(Error::LimitReached);
                    }
                    next.failures.push(FailureCount {
                        operation: failure.operation,
                        device_id: failure.device_id.clone(),
                        consecutive: 0,
                        total: 0,
                    });
                    next.failures.len() - 1
                };
                let count = &mut next.failures[at];
                count.total = count.total.saturating_add(1);
                count.consecutive = count.consecutive.saturating_add(1);
                next.total_failures = next.total_failures.saturating_add(1);
                let exhausted = count.consecutive >= self.policy.maximum_consecutive_failures
                    || next.total_failures >= self.policy.maximum_total_failures;
                let cause = Cause::Equipment {
                    failure: failure.clone(),
                };
                if failure.uncertain
                    || matches!(
                        failure.operation,
                        Operation::Slew | Operation::Capture | Operation::Other
                    )
                {
                    next.stop(cause, now, conditions.motion);
                } else if exhausted {
                    next.stop(Cause::FailureBudget {}, now, conditions.motion);
                } else if matches!(self.phase, Phase::Acquiring {}) {
                    next.hold(cause, now, conditions.motion);
                } else {
                    next.stop(cause, now, conditions.motion);
                }
            }
            Event::BeginRecovery { attempt_id } => {
                let Phase::Holding { hold } = &self.phase else {
                    return Err(Error::WrongPhase);
                };
                if !id(attempt_id)
                    || now < hold.retry_at_ms
                    || conditions.motion != Motion::Permitted
                {
                    return Err(Error::WrongPhase);
                }
                if self.probes_spent >= self.policy.maximum_probes {
                    next.stop(Cause::ProbeBudget {}, now, conditions.motion);
                } else {
                    let mut hold = hold.clone();
                    hold.probes += 1;
                    next.probes_spent += 1;
                    let deadline_ms = now
                        .saturating_add(self.policy.operation_timeout_ms)
                        .min(hold.expires_at_ms);
                    next.phase = Phase::Recovering {
                        hold,
                        attempt_id: attempt_id.clone(),
                        started_at_ms: now,
                        deadline_ms,
                    };
                }
            }
            Event::RecoveryCompleted { attempt_id, result } => {
                let Phase::Recovering {
                    hold,
                    attempt_id: pending,
                    started_at_ms,
                    ..
                } = &self.phase
                else {
                    return Err(Error::WrongPhase);
                };
                if pending != attempt_id {
                    return Err(Error::WrongPhase);
                }
                if matches!(result, RecoveryResult::Uncertain {}) {
                    next.stop(Cause::RecoveryUncertain {}, now, conditions.motion);
                } else {
                    let good = match (&hold.cause, result) {
                        (Cause::Quality { context }, RecoveryResult::Quality { sample }) => {
                            if context != &sample.context {
                                return Err(Error::ChangedReference);
                            }
                            next.sample(sample, now)?;
                            // Evidence must have been acquired after this attempt began.
                            if sample.observed_at_ms < *started_at_ms {
                                return Err(Error::StaleEvidence);
                            }
                            sample.verdict == Verdict::ConfirmedGood
                        }
                        (Cause::Equipment { .. }, RecoveryResult::EquipmentVerified {}) => true,
                        (_, RecoveryResult::Failed {}) => false,
                        _ => return Err(Error::WrongPhase),
                    };
                    let mut hold = hold.clone();
                    hold.good_probes = if good { hold.good_probes + 1 } else { 0 };
                    let needed = if matches!(hold.cause, Cause::Quality { .. }) {
                        self.policy.good_probes
                    } else {
                        1
                    };
                    if hold.good_probes >= needed {
                        if let Cause::Equipment { failure } = &hold.cause {
                            for count in &mut next.failures {
                                if count.operation == failure.operation
                                    && count.device_id == failure.device_id
                                {
                                    count.consecutive = 0;
                                }
                            }
                        }
                        next.phase = Phase::Acquiring {};
                        next.consecutive_bad = 0;
                    } else if next.probes_spent >= self.policy.maximum_probes {
                        next.stop(Cause::ProbeBudget {}, now, conditions.motion);
                    } else {
                        hold.retry_at_ms = now.saturating_add(self.policy.cooldown_ms);
                        next.phase = Phase::Holding { hold };
                    }
                }
            }
            _ => return Err(Error::WrongPhase),
        }
        Ok(next)
    }

    fn weather_transition(
        &self,
        next: &mut Self,
        now: u64,
        conditions: Conditions,
        event: &Event,
    ) -> Result<bool, Error> {
        let weather = self.policy.weather.as_ref().ok_or(Error::InvalidInput)?;
        let interrupted = matches!(event, Event::WeatherInterrupted { .. });
        let clear = conditions.safety == Safety::Safe && conditions.motion == Motion::Permitted;
        if let Phase::WeatherHolding {
            cause,
            started_at_ms,
            stable_since_ms,
        } = &self.phase
        {
            if next.weather_hold_ms >= weather.maximum_hold_ms
                || now >= self.policy.latest_resume_ms
            {
                next.stop(Cause::HoldExpired {}, now, conditions.motion);
                return Ok(true);
            }
            let since = if !clear || interrupted {
                None
            } else if now - self.last_event_ms > self.policy.evidence_max_age_ms {
                Some(now)
            } else {
                Some(stable_since_ms.unwrap_or(now))
            };
            if matches!(event, Event::ResumeWeather {}) {
                if !clear || since.is_none_or(|since| now - since < weather.stable_safe_ms) {
                    return Err(Error::WrongPhase);
                }
                next.phase = Phase::Acquiring {};
            } else if matches!(event, Event::Tick {} | Event::WeatherInterrupted { .. }) {
                next.phase = Phase::WeatherHolding {
                    cause: cause.clone(),
                    started_at_ms: *started_at_ms,
                    stable_since_ms: since,
                };
            } else {
                return Err(Error::WrongPhase);
            }
            return Ok(true);
        }
        if !clear || interrupted {
            // A retry already in flight is uncertain, not a weather-resumable boundary.
            if !matches!(self.phase, Phase::Acquiring {}) {
                next.stop(Cause::RecoveryUncertain {}, now, conditions.motion);
                return Ok(true);
            }
            if self.weather_interruptions >= weather.maximum_interruptions
                || self.weather_hold_ms >= weather.maximum_hold_ms
            {
                next.stop(Cause::HoldExpired {}, now, conditions.motion);
                return Ok(true);
            }
            next.weather_interruptions += 1;
            let enclosure = conditions.motion != Motion::Permitted
                || matches!(event, Event::WeatherInterrupted { enclosure: true });
            next.phase = Phase::WeatherHolding {
                cause: if enclosure {
                    Cause::Enclosure {}
                } else {
                    Cause::Safety {}
                },
                started_at_ms: now,
                stable_since_ms: None,
            };
            return Ok(true);
        }
        Ok(false)
    }

    fn sample(&mut self, sample: &QualitySample, now: u64) -> Result<(), Error> {
        sample.context.validate()?;
        if !id(&sample.capture_id)
            || sample.rig_id != self.identity.rig_id
            || sample.configuration_id != self.identity.configuration_id
        {
            return Err(Error::InvalidInput);
        }
        if sample.observed_at_ms < self.identity.starts_at_ms
            || sample.observed_at_ms > now
            || now - sample.observed_at_ms > self.policy.evidence_max_age_ms
            || self
                .last_quality_ms
                .is_some_and(|old| sample.observed_at_ms <= old)
        {
            return Err(Error::StaleEvidence);
        }
        self.last_quality_ms = Some(sample.observed_at_ms);
        Ok(())
    }

    fn hold(&mut self, cause: Cause, now: u64, motion: Motion) {
        let expires = now
            .saturating_add(
                self.policy
                    .maximum_hold_ms
                    .saturating_sub(self.total_hold_ms),
            )
            .min(self.policy.latest_resume_ms)
            .min(self.identity.ends_at_ms);
        if now >= expires {
            self.stop(Cause::HoldExpired {}, now, motion);
        } else {
            self.phase = Phase::Holding {
                hold: Hold {
                    cause,
                    started_at_ms: now,
                    retry_at_ms: now.saturating_add(self.policy.cooldown_ms),
                    expires_at_ms: expires,
                    probes: 0,
                    good_probes: 0,
                },
            };
        }
    }

    fn stop(&mut self, cause: Cause, now: u64, motion: Motion) {
        if !self.policy.park_on_stop {
            self.phase = Phase::Stopped {
                cause,
                shutdown: Shutdown::NoParkRequested,
            };
        } else if motion != Motion::Permitted {
            self.phase = Phase::Stopped {
                cause,
                shutdown: Shutdown::MotionBlocked,
            };
        } else {
            self.phase = Phase::Stopping {
                cause,
                deadline_ms: now.saturating_add(self.policy.operation_timeout_ms),
                park_attempt_id: None,
            };
        }
    }

    fn shutdown(&mut self, now: u64, motion: Motion, event: &Event) -> Result<(), Error> {
        let Phase::Stopping {
            cause,
            deadline_ms,
            park_attempt_id,
        } = self.phase.clone()
        else {
            return Err(Error::WrongPhase);
        };
        if let Event::ParkCompleted { attempt_id, result } = event {
            if park_attempt_id.as_ref() != Some(attempt_id) {
                return Err(Error::WrongPhase);
            }
            self.phase = Phase::Stopped {
                cause,
                shutdown: match result {
                    ParkResult::Parked => Shutdown::Parked,
                    ParkResult::Failed => Shutdown::ParkFailed,
                    ParkResult::Uncertain => Shutdown::ParkUncertain,
                },
            };
        } else if motion != Motion::Permitted {
            self.phase = Phase::Stopped {
                cause,
                shutdown: if park_attempt_id.is_some() {
                    Shutdown::ParkUncertain
                } else {
                    Shutdown::MotionBlocked
                },
            };
        } else if now >= deadline_ms {
            self.phase = Phase::Stopped {
                cause,
                shutdown: Shutdown::ParkUncertain,
            };
        } else if let Event::BeginPark { attempt_id } = event {
            if !id(attempt_id) || park_attempt_id.is_some() {
                return Err(Error::WrongPhase);
            }
            self.phase = Phase::Stopping {
                cause,
                deadline_ms,
                park_attempt_id: Some(attempt_id.clone()),
            };
        } else if !matches!(event, Event::Tick {} | Event::StopNight {}) {
            return Err(Error::WrongPhase);
        }
        Ok(())
    }
}
