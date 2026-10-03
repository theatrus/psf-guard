use super::*;

/// Deduplicated immutable policy sources plus exact goal bindings. Frozen in a
/// program; changing mutable settings cannot rewrite an outstanding grant.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ProgramPreferences {
    pub schema_version: u32,
    pub policies: BTreeMap<String, ResolvedPolicy>,
    pub bindings: BTreeMap<String, String>,
}

impl ProgramPreferences {
    pub fn validate(&self, assignment: &crate::Assignment) -> Result<(), Error> {
        if self.schema_version != VERSION
            || self.bindings.len() != assignment.goals.len()
            || self.policies.is_empty()
            || self.policies.len() > assignment.goals.len()
            || self.policies.keys().any(|id| !crate::valid_id(id))
            || assignment.goals.iter().any(|g| {
                !self
                    .bindings
                    .get(&g.id)
                    .is_some_and(|id| self.policies.contains_key(id))
            })
            || self
                .policies
                .keys()
                .any(|id| !self.bindings.values().any(|v| v == id))
        {
            return Err(Error::InvalidCandidates);
        }
        for policy in self.policies.values() {
            policy.policy.validate()?;
        }
        Ok(())
    }

    pub fn for_goals(&self) -> BTreeMap<String, ResolvedPolicy> {
        self.bindings
            .iter()
            .filter_map(|(goal, id)| self.policies.get(id).map(|p| (goal.clone(), p.clone())))
            .collect()
    }
}
