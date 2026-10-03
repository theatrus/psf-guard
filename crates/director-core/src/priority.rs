//! Explainable observing preferences. This additive preview API does not change
//! existing grants, the wire contract, or equipment dispatch authority.

use crate::{Decision, Request};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
mod program;
mod selection;
pub use program::ProgramPreferences;
pub use selection::{preview, ActiveGoal, Candidate, Contribution, Ranking, Score};

pub const VERSION: u32 = 1;
pub const SCALE: u16 = 10_000;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Factor {
    Importance,
    WindowUrgency,
    Altitude,
    MoonOpportunity,
    Completion,
    Efficiency,
    Continuity,
}

impl Factor {
    pub const ALL: [Self; 7] = [
        Self::Importance,
        Self::WindowUrgency,
        Self::Altitude,
        Self::MoonOpportunity,
        Self::Completion,
        Self::Efficiency,
        Self::Continuity,
    ];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Balanced,
    FinishGoals,
    BestConditions,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    /// Relative weights, not percentages. Zero disables a factor.
    #[serde(deserialize_with = "read_weights")]
    pub weights: BTreeMap<Factor, u16>,
    /// User intent, independent of the legacy TS ordinal. Zero is not a veto.
    pub importance: u16,
    pub minimum_dwell_ms: u64,
    /// Challenger must improve the normalized score by more than this margin.
    pub switch_margin: u16,
}

impl Policy {
    pub fn preset(preset: Preset) -> Self {
        let weights = match preset {
            Preset::Balanced => [30, 25, 15, 15, 5, 5, 5],
            Preset::FinishGoals => [25, 15, 10, 10, 25, 5, 10],
            Preset::BestConditions => [20, 15, 30, 25, 0, 5, 5],
        };
        Self {
            weights: Factor::ALL.into_iter().zip(weights).collect(),
            importance: 50,
            minimum_dwell_ms: 600_000,
            switch_margin: 500,
        }
    }

    pub fn validate(&self) -> Result<(), Error> {
        if self.weights.len() != Factor::ALL.len()
            || Factor::ALL.iter().any(|f| !self.weights.contains_key(f))
            || self.weights.values().any(|w| *w > 1000)
            || self.weights.values().all(|w| *w == 0)
            || self.importance > 100
            || self.minimum_dwell_ms > 86_400_000
            || self.switch_margin > SCALE
        {
            return Err(Error::InvalidPolicy);
        }
        Ok(())
    }
}

/// Import only. New intent uses the full 0..=100 range; legacy unknowns are Normal.
pub fn ts_importance(priority: Option<i64>) -> u16 {
    match priority {
        Some(0) => 25,
        Some(2) => 75,
        _ => 50,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Global,
    Site,
    Rig,
    Project,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub scope: Scope,
    pub id: String,
    pub revision: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Overrides {
    #[serde(default, deserialize_with = "read_weights")]
    pub weights: BTreeMap<Factor, u16>,
    pub importance: Option<u16>,
    pub minimum_dwell_ms: Option<u64>,
    pub switch_margin: Option<u16>,
}

impl Overrides {
    pub fn apply_to(&self, mut policy: Policy) -> Result<Policy, Error> {
        policy
            .weights
            .extend(self.weights.iter().map(|(k, v)| (*k, *v)));
        if let Some(value) = self.importance {
            policy.importance = value;
        }
        if let Some(value) = self.minimum_dwell_ms {
            policy.minimum_dwell_ms = value;
        }
        if let Some(value) = self.switch_margin {
            policy.switch_margin = value;
        }
        policy.validate()?;
        Ok(policy)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub source: Source,
    pub overrides: Overrides,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub weights: BTreeMap<Factor, Source>,
    pub importance: Source,
    pub minimum_dwell_ms: Source,
    pub switch_margin: Source,
}

/// Construct only with resolve: retain both editable intent and effective sources.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ResolvedPolicy {
    schema_version: u32,
    policy: Policy,
    provenance: Provenance,
    global: Policy,
    global_source: Source,
    layers: Vec<Layer>,
}

impl<'de> Deserialize<'de> for ResolvedPolicy {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            schema_version: u32,
            policy: Policy,
            provenance: Provenance,
            global: Policy,
            global_source: Source,
            layers: Vec<Layer>,
        }
        let wire = Wire::deserialize(deserializer)?;
        let resolved = resolve(wire.global, wire.global_source, &wire.layers)
            .map_err(|_| serde::de::Error::custom("invalid observing policy"))?;
        if wire.schema_version != VERSION
            || wire.policy != resolved.policy
            || wire.provenance != resolved.provenance
        {
            return Err(serde::de::Error::custom(
                "observing policy provenance mismatch",
            ));
        }
        Ok(resolved)
    }
}

impl ResolvedPolicy {
    pub fn policy(&self) -> &Policy {
        &self.policy
    }
    pub fn provenance(&self) -> &Provenance {
        &self.provenance
    }
}

pub fn resolve(global: Policy, source: Source, layers: &[Layer]) -> Result<ResolvedPolicy, Error> {
    global.validate()?;
    if source.scope != Scope::Global || !valid_source(&source) || layers.len() > 3 {
        return Err(Error::InvalidHierarchy);
    }
    let mut provenance = Provenance {
        weights: Factor::ALL
            .into_iter()
            .map(|f| (f, source.clone()))
            .collect(),
        importance: source.clone(),
        minimum_dwell_ms: source.clone(),
        switch_margin: source.clone(),
    };
    let mut policy = global.clone();
    let mut previous = source.scope;
    for layer in layers {
        if layer.source.scope <= previous || !valid_source(&layer.source) {
            return Err(Error::InvalidHierarchy);
        }
        previous = layer.source.scope;
        for (factor, weight) in &layer.overrides.weights {
            policy.weights.insert(*factor, *weight);
            provenance.weights.insert(*factor, layer.source.clone());
        }
        if let Some(value) = layer.overrides.importance {
            policy.importance = value;
            provenance.importance = layer.source.clone();
        }
        if let Some(value) = layer.overrides.minimum_dwell_ms {
            policy.minimum_dwell_ms = value;
            provenance.minimum_dwell_ms = layer.source.clone();
        }
        if let Some(value) = layer.overrides.switch_margin {
            policy.switch_margin = value;
            provenance.switch_margin = layer.source.clone();
        }
        policy.validate()?;
    }
    Ok(ResolvedPolicy {
        schema_version: VERSION,
        policy,
        provenance,
        global,
        global_source: source,
        layers: layers.to_vec(),
    })
}

fn valid_source(source: &Source) -> bool {
    crate::valid_id(&source.id) && source.revision > 0
}

fn read_weights<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<Factor, u16>, D::Error> {
    struct UniqueWeights;
    impl<'de> serde::de::Visitor<'de> for UniqueWeights {
        type Value = BTreeMap<Factor, u16>;
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("unique observing preference weights")
        }
        fn visit_map<M: serde::de::MapAccess<'de>>(
            self,
            mut map: M,
        ) -> Result<Self::Value, M::Error> {
            let mut weights = BTreeMap::new();
            while let Some((factor, weight)) = map.next_entry::<Factor, u16>()? {
                if weights.insert(factor, weight).is_some() {
                    return Err(serde::de::Error::custom(
                        "duplicate observing preference weight",
                    ));
                }
            }
            Ok(weights)
        }
    }
    deserializer.deserialize_map(UniqueWeights)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPolicy,
    InvalidHierarchy,
    InvalidCandidates,
    InvalidActiveGoal,
    Planning(crate::Error),
}

fn gated(decision: Decision) -> Ranking {
    Ranking {
        decision,
        candidates: vec![],
        retained_active: false,
    }
}
