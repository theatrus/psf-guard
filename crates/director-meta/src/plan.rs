//! The editable acquisition plan of a global project: what bandpasses and
//! depth it wants, and which rig shoots which of them with which template.
//! Activation freezes intent from it; the draft itself authorizes nothing.

use super::*;
use psf_guard_director_core::MAX_REQUEST_BYTES;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Goal {
    /// Accepted integration in hours; each rig converts through its exposure.
    Hours { value: f64 },
    /// Accepted frames per rig, the way Target Scheduler counts.
    Frames { value: u32 },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Objective {
    pub id: Uuid,
    /// Core bandpass identity, such as `h_alpha` or `red`.
    pub bandpass_id: String,
    /// `faint_detail`, `unsaturated_stars` or another short slug.
    pub purpose: String,
    pub goal: Goal,
    pub priority: u32,
}

/// The Target Scheduler template a contribution binds to, copied so the plan
/// still reads the same if the rig database edits or drops the template.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TemplateChoice {
    #[serde(deserialize_with = "Option::deserialize")]
    pub template_guid: Option<Uuid>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub template_id: Option<i64>,
    pub name: String,
    pub filter_name: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub gain: Option<i32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub offset: Option<i32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub bin: Option<i32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub readout_mode: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moon: Option<psf_guard_director_core::moon::MoonPolicy>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub id: Uuid,
    pub objective_id: Uuid,
    pub rig_id: Uuid,
    pub template: TemplateChoice,
    pub exposure_seconds: f64,
    /// Panel IDs from the framing draft; empty means every panel.
    pub panel_ids: Vec<String>,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlanDraft {
    pub project_id: Uuid,
    pub revision: u64,
    pub objectives: Vec<Objective>,
    pub contributions: Vec<Contribution>,
    pub updated_at_ms: u64,
}

impl PlanDraft {
    pub fn empty(project_id: Uuid, now_ms: u64) -> Self {
        Self {
            project_id,
            revision: 0,
            objectives: vec![],
            contributions: vec![],
            updated_at_ms: now_ms,
        }
    }
}

const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01
const MAX_OBJECTIVES: usize = 64;
const MAX_CONTRIBUTIONS: usize = 256;

fn slug_ok(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn text_ok(value: &str, max: usize) -> bool {
    value.len() <= max && value.trim() == value && !value.chars().any(char::is_control)
}

pub(crate) fn validate_plan(plan: &PlanDraft) -> Result<(), Error> {
    valid_id(plan.project_id)?;
    if plan.updated_at_ms > MAX_TIME_MS
        || plan.objectives.len() > MAX_OBJECTIVES
        || plan.contributions.len() > MAX_CONTRIBUTIONS
    {
        return Err(Error::InvalidInput);
    }
    let mut objective_ids = std::collections::BTreeSet::new();
    for objective in &plan.objectives {
        valid_id(objective.id)?;
        if !objective_ids.insert(objective.id)
            || !slug_ok(&objective.bandpass_id, 64)
            || !slug_ok(&objective.purpose, 64)
            || objective.priority > 1000
        {
            return Err(Error::InvalidInput);
        }
        match objective.goal {
            Goal::Hours { value } => {
                if !value.is_finite() || value <= 0.0 || value > 10_000.0 {
                    return Err(Error::InvalidInput);
                }
            }
            Goal::Frames { value } => {
                if value == 0 || value > 1_000_000 {
                    return Err(Error::InvalidInput);
                }
            }
        }
    }
    let mut contribution_ids = std::collections::BTreeSet::new();
    for contribution in &plan.contributions {
        valid_id(contribution.id)?;
        valid_id(contribution.rig_id)?;
        if !contribution_ids.insert(contribution.id)
            || !objective_ids.contains(&contribution.objective_id)
            || !contribution.exposure_seconds.is_finite()
            || contribution.exposure_seconds <= 0.0
            || contribution.exposure_seconds > 86_400.0
            || contribution.panel_ids.len() > 256
            || contribution
                .panel_ids
                .iter()
                .any(|id| !text_ok(id, 32) || id.is_empty())
        {
            return Err(Error::InvalidInput);
        }
        let template = &contribution.template;
        if let Some(guid) = template.template_guid {
            valid_id(guid)?;
        }
        if !text_ok(&template.name, 256)
            || !text_ok(&template.filter_name, 128)
            || template.filter_name.is_empty()
            || template.template_id.is_some_and(|id| id <= 0)
            || template.bin.is_some_and(|bin| bin <= 0)
            || template
                .moon
                .as_ref()
                .is_some_and(|policy| policy.validate().is_err())
        {
            return Err(Error::InvalidInput);
        }
    }
    Ok(())
}

impl MetaStore {
    pub fn plan_draft(&self, project: Uuid) -> Result<Option<PlanDraft>, Error> {
        read_plan(&self.connection, project)
    }

    /// Save with compare-and-set; see `save_framing_draft` for the contract.
    pub fn save_plan_draft(
        &mut self,
        plan: &PlanDraft,
        expected_revision: u64,
    ) -> Result<PlanDraft, Error> {
        validate_plan(plan)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Project, plan.project_id)?.is_none() {
            return Err(Error::NotFound);
        }
        for contribution in &plan.contributions {
            if read_named(&tx, Kind::Rig, contribution.rig_id)?.is_none() {
                return Err(Error::NotFound);
            }
        }
        let stored = read_plan(&tx, plan.project_id)?;
        let current = stored.as_ref().map_or(0, |value| value.revision);
        if current != expected_revision {
            return Err(Error::Conflict);
        }
        let mut next = plan.clone();
        next.revision = current;
        if let Some(stored) = stored {
            let mut same = stored.clone();
            same.updated_at_ms = next.updated_at_ms;
            if same == next {
                tx.commit()?;
                return Ok(stored);
            }
        }
        next.revision = current.checked_add(1).ok_or(Error::Conflict)?;
        let payload = super::configuration::encode(&next)?;
        tx.execute(
            "INSERT INTO plan_draft(project_id,revision,payload) VALUES(?1,?2,?3)
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

pub(crate) fn read_plan(conn: &Connection, project: Uuid) -> Result<Option<PlanDraft>, Error> {
    valid_id(project)?;
    let row: Option<(i64, Vec<u8>)> = conn
        .query_row(
            "SELECT revision,substr(CAST(payload AS BLOB),1,?2) FROM plan_draft WHERE project_id=?1",
            params![project.to_string(), (MAX_REQUEST_BYTES + 1) as i64],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(revision, payload)| {
        let value: PlanDraft = super::configuration::decode(payload)?;
        validate_plan(&value).map_err(|_| Error::CorruptDatabase)?;
        if value.project_id != project || revision <= 0 || value.revision != revision as u64 {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}
