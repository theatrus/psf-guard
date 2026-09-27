//! Project intent shared by coordinator and executor. This is neither an
//! allocation nor acquisition authority; it contains no progress projection.

use crate::program::{self, Configuration, Recipe, Target};
use crate::{valid_id, MAX_GOALS, MAX_REQUEST_BYTES};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROJECT_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Objective {
    pub id: String,
    /// Semantic bandpass identity, not a rig's filter name or wheel position.
    pub bandpass_id: String,
    /// Explicit intent such as faint_detail or unsaturated_stars. Never inferred
    /// from exposure duration, filter name, or an existing catalog's grouping.
    pub purpose: String,
    pub target: Target,
    pub priority: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Contribution {
    pub id: String,
    pub objective_id: String,
    pub rig_id: String,
    /// Coordinator setup revision, not the native equipment fingerprint.
    pub setup_id: String,
    pub configuration_id: String,
    /// Rig-specific framing. A panel may differ from the objective's center.
    pub framing: Target,
    /// This explicitly maps the objective's bandpass to a native filter ID.
    pub recipe: Recipe,
    /// Required accepted frames for this contribution only. Different rigs or
    /// purposes never satisfy one another by adding counts or integration time.
    pub required_accepted_frames: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub schema_version: u32,
    pub project_id: String,
    /// Immutable intent snapshot identity; changed content needs another ID.
    pub id: String,
    pub objectives: Vec<Objective>,
    pub contributions: Vec<Contribution>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    TooLarge,
    InvalidJson,
    UnsupportedVersion,
    InvalidProject,
    InvalidObjective,
    InvalidContribution,
    ConflictingTarget,
    ConflictingRecipe,
    ConflictingSetup,
    ConflictingBandpass,
    UnknownContribution,
    SetupMismatch,
    Program(program::Error),
}

/// Own the snapshot so later source edits cannot change resolved intent.
#[derive(Clone, Debug)]
pub struct BoundProject {
    project: Project,
    contributions: BTreeMap<String, (usize, usize)>,
}

pub struct ResolvedContribution<'a> {
    pub objective: &'a Objective,
    pub contribution: &'a Contribution,
}

impl BoundProject {
    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(Error::TooLarge);
        }
        Self::new(serde_json::from_slice(bytes).map_err(|_| Error::InvalidJson)?)
    }

    pub fn new(project: Project) -> Result<Self, Error> {
        if serde_json::to_vec(&project)
            .map_err(|_| Error::InvalidJson)?
            .len()
            > MAX_REQUEST_BYTES
        {
            return Err(Error::TooLarge);
        }
        if project.schema_version != PROJECT_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        if !valid_id(&project.project_id)
            || !valid_id(&project.id)
            || project.objectives.is_empty()
            || project.objectives.len() > MAX_GOALS
            || project.contributions.is_empty()
            || project.contributions.len() > MAX_GOALS
        {
            return Err(Error::InvalidProject);
        }
        let mut objectives = BTreeMap::new();
        let mut targets = BTreeMap::new();
        for (index, objective) in project.objectives.iter().enumerate() {
            if !valid_id(&objective.id)
                || !valid_id(&objective.bandpass_id)
                || !valid_id(&objective.purpose)
                || objectives.insert(&objective.id, index).is_some()
            {
                return Err(Error::InvalidObjective);
            }
            remember_target(&mut targets, &objective.target)?;
        }
        let mut contributions = BTreeMap::new();
        let mut recipes = BTreeMap::new();
        let mut setups = BTreeMap::new();
        let mut bandpasses = BTreeMap::new();
        let mut used_objectives = vec![false; project.objectives.len()];
        for (index, contribution) in project.contributions.iter().enumerate() {
            let objective = *objectives
                .get(&contribution.objective_id)
                .ok_or(Error::InvalidContribution)?;
            if !valid_id(&contribution.id)
                || !valid_id(&contribution.rig_id)
                || !valid_id(&contribution.setup_id)
                || !valid_id(&contribution.configuration_id)
                || contribution.required_accepted_frames == 0
                || contributions
                    .insert(contribution.id.clone(), (objective, index))
                    .is_some()
            {
                return Err(Error::InvalidContribution);
            }
            remember_target(&mut targets, &contribution.framing)?;
            let recipe = &contribution.recipe;
            program::validate_recipe_shape(recipe).map_err(Error::Program)?;
            let setup = (&contribution.rig_id, &contribution.configuration_id);
            if setups
                .insert(&contribution.setup_id, setup)
                .is_some_and(|previous| previous != setup)
            {
                return Err(Error::ConflictingSetup);
            }
            let filter = (
                &contribution.rig_id,
                &contribution.configuration_id,
                &recipe.filter_id,
            );
            let bandpass = &project.objectives[objective].bandpass_id;
            if bandpasses
                .insert(filter, bandpass)
                .is_some_and(|previous| previous != bandpass)
            {
                return Err(Error::ConflictingBandpass);
            }
            // Recipe identity is rig/configuration scoped, just like its filter.
            let key = (
                &contribution.rig_id,
                &contribution.configuration_id,
                &recipe.id,
            );
            if recipes
                .insert(key, recipe)
                .is_some_and(|previous| previous != recipe)
            {
                return Err(Error::ConflictingRecipe);
            }
            used_objectives[objective] = true;
        }
        if used_objectives.contains(&false) {
            return Err(Error::InvalidObjective);
        }
        Ok(Self {
            project,
            contributions,
        })
    }

    pub fn snapshot(&self) -> &Project {
        &self.project
    }

    /// Resolve explicit intent and check the selected immutable setup. This does
    /// not establish FOV/quality equivalence, visibility, progress, or permission.
    pub fn resolve(
        &self,
        contribution_id: &str,
        setup_id: &str,
        configuration: &Configuration,
    ) -> Result<ResolvedContribution<'_>, Error> {
        let &(objective, contribution) = self
            .contributions
            .get(contribution_id)
            .ok_or(Error::UnknownContribution)?;
        let contribution = &self.project.contributions[contribution];
        if contribution.setup_id != setup_id
            || contribution.rig_id != configuration.rig_id
            || contribution.configuration_id != configuration.id
        {
            return Err(Error::SetupMismatch);
        }
        program::validate_configuration(configuration).map_err(Error::Program)?;
        program::validate_recipe(&contribution.recipe, configuration).map_err(Error::Program)?;
        Ok(ResolvedContribution {
            objective: &self.project.objectives[objective],
            contribution,
        })
    }
}

fn remember_target<'a>(
    targets: &mut BTreeMap<&'a str, &'a Target>,
    target: &'a Target,
) -> Result<(), Error> {
    program::validate_target(target).map_err(Error::Program)?;
    if targets
        .insert(&target.id, target)
        .is_some_and(|previous| previous != target)
    {
        return Err(Error::ConflictingTarget);
    }
    Ok(())
}
