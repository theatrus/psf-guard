//! What an activation pushed into which rig database: the record that lets
//! a later activation update the same rows and lets the program endpoint
//! know what is live. It is history, not authority; the rig databases hold
//! the rows themselves.

use super::*;
use psf_guard_director_core::MAX_REQUEST_BYTES;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActivatedTarget {
    pub panel_id: String,
    pub target_guid: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActivatedPlan {
    pub contribution_id: Uuid,
    pub objective_id: Uuid,
    pub target_guid: Uuid,
    pub exposureplan_guid: Uuid,
    pub required_frames: u32,
    /// What the objective asked for when this was activated, so the rig's
    /// program keeps serving the reviewed activation while the plan is
    /// edited. Absent in records written before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub intent: Option<PlanIntent>,
}

/// The objective's band, purpose and priority as activated.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlanIntent {
    pub bandpass_id: String,
    pub purpose: String,
    pub priority: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ActivatedRig {
    pub rig_id: Uuid,
    pub catalog_id: Uuid,
    pub project_guid: Uuid,
    pub profile_id: String,
    pub targets: Vec<ActivatedTarget>,
    pub plans: Vec<ActivatedPlan>,
}

/// A rig turned off in the plan whose Target Scheduler project activation
/// set Inactive. Turning the rig on again sets the project Active; one the
/// operator made Inactive is not listed and stays as they left it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InactiveRig {
    pub rig_id: Uuid,
    pub catalog_id: Uuid,
    pub project_guid: Uuid,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Activation {
    pub project_id: Uuid,
    /// Increments on every apply; not compare-and-set, since an apply is the
    /// reviewed result of a preview digest.
    pub revision: u64,
    pub framing_revision: u64,
    pub plan_revision: u64,
    pub coordinator_instance_id: Uuid,
    pub applied_at_ms: u64,
    pub rigs: Vec<ActivatedRig>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inactive_rigs: Vec<InactiveRig>,
}

impl Activation {
    /// Whether the store would take this record, size included, so a caller
    /// can check before it writes anything the record must describe.
    pub fn validate(&self) -> Result<(), Error> {
        validate_activation(self)?;
        super::configuration::encode(self).map(|_| ())
    }
}

fn validate_activation(activation: &Activation) -> Result<(), Error> {
    valid_id(activation.project_id)?;
    valid_id(activation.coordinator_instance_id)?;
    if activation.rigs.len() > 64 || activation.inactive_rigs.len() > 64 {
        return Err(Error::InvalidInput);
    }
    for rig in &activation.inactive_rigs {
        valid_id(rig.rig_id)?;
        valid_id(rig.catalog_id)?;
        valid_id(rig.project_guid)?;
    }
    for rig in &activation.rigs {
        valid_id(rig.rig_id)?;
        valid_id(rig.catalog_id)?;
        valid_id(rig.project_guid)?;
        if rig.profile_id.is_empty()
            || rig.profile_id.len() > 128
            || rig.targets.len() > 256
            || rig.plans.len() > 4096
        {
            return Err(Error::InvalidInput);
        }
        for target in &rig.targets {
            valid_id(target.target_guid)?;
            if target.panel_id.is_empty() || target.panel_id.len() > 32 {
                return Err(Error::InvalidInput);
            }
        }
        for plan in &rig.plans {
            valid_id(plan.contribution_id)?;
            valid_id(plan.objective_id)?;
            valid_id(plan.target_guid)?;
            valid_id(plan.exposureplan_guid)?;
            if let Some(intent) = &plan.intent
                && (intent.bandpass_id.is_empty()
                    || intent.bandpass_id.len() > 64
                    || intent.purpose.len() > 64
                    || intent.priority > 1000)
            {
                return Err(Error::InvalidInput);
            }
        }
    }
    Ok(())
}

impl MetaStore {
    pub fn activation(&self, project: Uuid) -> Result<Option<Activation>, Error> {
        read_activation(&self.connection, project)
    }

    /// Every recorded activation that gave this rig work, oldest project first.
    /// A scan of the activation table; coordinators hold tens of projects, not
    /// millions, and the program endpoint calls this once per pull.
    pub fn activations_for_rig(&self, rig: Uuid) -> Result<Vec<Activation>, Error> {
        valid_id(rig)?;
        let mut statement = self
            .connection
            .prepare("SELECT project_id FROM activation ORDER BY project_id")?;
        let projects = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut found = Vec::new();
        for project in projects {
            if let Some(activation) = read_activation(&self.connection, parse_id(&project)?)?
                && activation.rigs.iter().any(|entry| entry.rig_id == rig)
            {
                found.push(activation);
            }
        }
        Ok(found)
    }

    /// Record an applied activation. The stored revision advances by one
    /// regardless of the caller's value.
    pub fn record_activation(&mut self, activation: &Activation) -> Result<Activation, Error> {
        validate_activation(activation)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Project, activation.project_id)?.is_none() {
            return Err(Error::NotFound);
        }
        let current = read_activation(&tx, activation.project_id)?.map_or(0, |a| a.revision);
        let mut next = activation.clone();
        next.revision = current.checked_add(1).ok_or(Error::Conflict)?;
        let payload = super::configuration::encode(&next)?;
        tx.execute(
            "INSERT INTO activation(project_id,revision,payload) VALUES(?1,?2,?3)
             ON CONFLICT(project_id) DO UPDATE SET revision=excluded.revision, payload=excluded.payload",
            params![
                next.project_id.to_string(),
                i64::try_from(next.revision).map_err(|_| Error::Conflict)?,
                payload
            ],
        )?;
        tx.commit()?;
        Ok(next)
    }
}

pub(crate) fn read_activation(
    conn: &Connection,
    project: Uuid,
) -> Result<Option<Activation>, Error> {
    valid_id(project)?;
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT revision,substr(CAST(payload AS BLOB),1,?2) FROM activation WHERE project_id=?1",
            params![project.to_string(), (MAX_REQUEST_BYTES + 1) as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(revision, payload)| {
        let value: Activation = super::configuration::decode(payload)?;
        validate_activation(&value).map_err(|_| Error::CorruptDatabase)?;
        if value.project_id != project || revision <= 0 || value.revision != revision as u64 {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}
