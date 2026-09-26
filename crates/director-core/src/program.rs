//! Immutable target/recipe bindings for an allocated observing program.
//! Validation and preparation are shared by simulation and execution hosts.
//! This is not authentication, persistence, or a hardware dispatch permit.

use crate::preparation::{Context, Estimates, Preparation};
use crate::{valid_id, Assignment, Goal, Request, State, CONTRACT_VERSION, MAX_REQUEST_BYTES};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const PROGRAM_VERSION: u32 = 1;
/// Integer milliarcseconds keep coordinate identity exact across JSON hosts.
pub const MAS_PER_DEGREE: u32 = 3_600_000;
const FULL_CIRCLE: u32 = 360 * MAS_PER_DEGREE;
const POLE: i32 = 90 * MAS_PER_DEGREE as i32;
const MAX_CAPABILITIES: usize = 256;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub id: String,
    pub name: String,
    pub icrs_ra_mas: u32,
    pub icrs_dec_mas: i32,
    /// None means no requested field rotation, not zero degrees.
    #[serde(deserialize_with = "Option::deserialize")]
    pub position_angle_mas: Option<u32>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct Binning {
    pub x: i16,
    pub y: i16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "support", rename_all = "snake_case", deny_unknown_fields)]
pub enum Control {
    Unsupported {},
    Range { minimum: i32, maximum: i32 },
    Values { values: Vec<i32> },
}

impl Control {
    fn validate(&self) -> bool {
        match self {
            Self::Unsupported {} => true,
            Self::Range { minimum, maximum } => *minimum >= 0 && maximum >= minimum,
            Self::Values { values } => {
                !values.is_empty()
                    && values.len() <= MAX_CAPABILITIES
                    && values.iter().all(|value| *value >= 0)
                    && values.iter().collect::<BTreeSet<_>>().len() == values.len()
            }
        }
    }

    fn accepts(&self, value: Option<i32>) -> bool {
        match (self, value) {
            (Self::Unsupported {}, None) => true,
            (Self::Range { minimum, maximum }, Some(value)) => {
                value >= *minimum && value <= *maximum
            }
            (Self::Values { values }, Some(value)) => values.contains(&value),
            _ => false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub id: String,
    /// No wheel means one fixed optical filter with a null position.
    #[serde(deserialize_with = "Option::deserialize")]
    pub position: Option<i16>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub rig_id: String,
    pub id: String,
    pub camera_id: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub filter_wheel_id: Option<String>,
    pub filters: Vec<Filter>,
    pub binning_modes: Vec<Binning>,
    pub readout_modes: Vec<i16>,
    pub gain: Control,
    pub offset: Control,
    pub exposure_min_ms: u64,
    pub exposure_max_ms: u64,
    pub enable_slew_center: bool,
    pub dither_every: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub id: String,
    pub exposure_ms: u64,
    pub filter_id: String,
    pub binning: Binning,
    /// None is allowed only when the control is unsupported, never "use current".
    #[serde(deserialize_with = "Option::deserialize")]
    pub gain: Option<i32>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub offset: Option<i32>,
    pub readout_mode: i16,
    #[serde(deserialize_with = "Option::deserialize")]
    pub dither_override: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub goal_id: String,
    pub target_id: String,
    pub recipe_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub schema_version: u32,
    pub assignment: Assignment,
    pub configuration: Configuration,
    pub targets: Vec<Target>,
    pub recipes: Vec<Recipe>,
    pub bindings: Vec<Binding>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    TooLarge,
    InvalidJson,
    UnsupportedVersion,
    InvalidConfiguration,
    ConfigurationMismatch,
    InvalidTarget,
    InvalidRecipe,
    InvalidBinding,
    UnknownGoal,
    RotationUnavailable,
    InvalidLocalState,
    Planning(crate::Error),
    Preparation(crate::preparation::Error),
}

/// Local observations, never a replacement recipe or an alternative target.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalState {
    pub configuration: Configuration,
    #[serde(deserialize_with = "Option::deserialize")]
    pub previous_pointing: Option<PointingContext>,
    pub mount_parked: bool,
    pub rotator_connected: bool,
    pub filter_exposures_since_dither: u32,
}

/// A remembered name or ID alone cannot prove that framing is unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointingContext {
    pub configuration_id: String,
    pub target: Target,
}

/// Own the validated snapshot so callers cannot mutate it behind a binding.
#[derive(Clone, Debug)]
pub struct BoundProgram {
    program: Program,
    bindings: BTreeMap<String, (usize, usize, usize)>,
}

/// Resolved inputs, not a recommendation or authorization to expose.
pub struct ResolvedGoal<'a> {
    pub goal: &'a Goal,
    pub target: &'a Target,
    pub recipe: &'a Recipe,
    pub configuration: &'a Configuration,
}

impl BoundProgram {
    pub fn from_json(bytes: &[u8], state: &State) -> Result<Self, Error> {
        if bytes.len() > MAX_REQUEST_BYTES {
            return Err(Error::TooLarge);
        }
        let program = serde_json::from_slice(bytes).map_err(|_| Error::InvalidJson)?;
        Self::new(program, state)
    }

    pub fn new(program: Program, state: &State) -> Result<Self, Error> {
        if serde_json::to_vec(&program)
            .map_err(|_| Error::InvalidJson)?
            .len()
            > MAX_REQUEST_BYTES
        {
            return Err(Error::TooLarge);
        }
        if program.schema_version != PROGRAM_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        crate::validate(&Request {
            contract_version: CONTRACT_VERSION,
            assignment: program.assignment.clone(),
            state: state.clone(),
        })
        .map_err(Error::Planning)?;
        validate_configuration(&program.configuration)?;
        if program.configuration.id != program.assignment.configuration_id
            || program.configuration.rig_id != program.assignment.rig_id
            || state.rig_id != program.assignment.rig_id
            || state.configuration_id != program.assignment.configuration_id
        {
            return Err(Error::ConfigurationMismatch);
        }
        if program.targets.is_empty()
            || program.recipes.is_empty()
            || program.targets.len() > program.assignment.goals.len()
            || program.recipes.len() > program.assignment.goals.len()
            || program.bindings.len() != program.assignment.goals.len()
        {
            return Err(Error::InvalidBinding);
        }
        let mut targets = BTreeMap::new();
        for (index, target) in program.targets.iter().enumerate() {
            validate_target(target)?;
            if targets.insert(&target.id, index).is_some() {
                return Err(Error::InvalidTarget);
            }
        }
        let mut recipes = BTreeMap::new();
        for (index, recipe) in program.recipes.iter().enumerate() {
            validate_recipe(recipe, &program.configuration)?;
            if recipes.insert(&recipe.id, index).is_some() {
                return Err(Error::InvalidRecipe);
            }
        }
        let goals: BTreeMap<_, _> = program
            .assignment
            .goals
            .iter()
            .enumerate()
            .map(|(index, goal)| (&goal.id, index))
            .collect();
        let mut bindings = BTreeMap::new();
        let mut used_targets = BTreeSet::new();
        let mut used_recipes = BTreeSet::new();
        for binding in &program.bindings {
            let goal = *goals.get(&binding.goal_id).ok_or(Error::InvalidBinding)?;
            let target = *targets
                .get(&binding.target_id)
                .ok_or(Error::InvalidBinding)?;
            let recipe = *recipes
                .get(&binding.recipe_id)
                .ok_or(Error::InvalidBinding)?;
            if program.assignment.goals[goal].exposure_ms != program.recipes[recipe].exposure_ms
                || bindings
                    .insert(binding.goal_id.clone(), (goal, target, recipe))
                    .is_some()
            {
                return Err(Error::InvalidBinding);
            }
            used_targets.insert(target);
            used_recipes.insert(recipe);
        }
        if used_targets.len() != targets.len() || used_recipes.len() != recipes.len() {
            return Err(Error::InvalidBinding);
        }
        Ok(Self { program, bindings })
    }

    pub fn snapshot(&self) -> &Program {
        &self.program
    }

    pub fn resolve(&self, goal_id: &str) -> Result<ResolvedGoal<'_>, Error> {
        let &(goal, target, recipe) = self.bindings.get(goal_id).ok_or(Error::UnknownGoal)?;
        Ok(ResolvedGoal {
            goal: &self.program.assignment.goals[goal],
            target: &self.program.targets[target],
            recipe: &self.program.recipes[recipe],
            configuration: &self.program.configuration,
        })
    }

    /// Build preparation only from this program's resolved IDs and settings.
    /// The ledger must supply progress-projected counts and retain this binding.
    pub fn preparation(
        &self,
        id: String,
        request: &Request,
        goal_id: &str,
        local: LocalState,
        estimates: Estimates,
    ) -> Result<Preparation, Error> {
        self.validate_projection(&request.assignment)?;
        Preparation::new(
            id,
            request,
            self.preparation_context(goal_id, local)?,
            estimates,
        )
        .map_err(Error::Preparation)
    }

    /// Resolve preparation parameters without making a scheduling decision.
    /// Durable hosts use this for idempotent begin retries with projected progress.
    pub fn preparation_context(&self, goal_id: &str, local: LocalState) -> Result<Context, Error> {
        if local.configuration != self.program.configuration {
            return Err(Error::ConfigurationMismatch);
        }
        let resolved = self.resolve(goal_id)?;
        let previous_target_id = match local.previous_pointing {
            Some(previous) => {
                if !valid_id(&previous.configuration_id) || !valid_id(&previous.target.id) {
                    return Err(Error::InvalidLocalState);
                }
                // Treat changed coordinates/rotation or equipment as a new target,
                // even if a coordinator retained the same stable target ID.
                (previous.configuration_id == resolved.configuration.id
                    && previous.target == *resolved.target)
                    .then_some(previous.target.id)
            }
            None => None,
        };
        if resolved.target.position_angle_mas.is_some()
            && (!local.rotator_connected || !resolved.configuration.enable_slew_center)
        {
            return Err(Error::RotationUnavailable);
        }
        Ok(Context {
            goal_id: goal_id.into(),
            target_id: resolved.target.id.clone(),
            recipe_id: resolved.recipe.id.clone(),
            previous_target_id,
            filter_id: resolved.recipe.filter_id.clone(),
            readout_mode: resolved.recipe.readout_mode,
            mount_parked: local.mount_parked,
            rotator_connected: local.rotator_connected
                && resolved.target.position_angle_mas.is_some(),
            enable_slew_center: resolved.configuration.enable_slew_center,
            dither_every: resolved.configuration.dither_every,
            dither_override: resolved.recipe.dither_override,
            filter_exposures_since_dither: local.filter_exposures_since_dither,
        })
    }

    /// The owning ledger supplies progress. Only counters may differ; a caller
    /// cannot use a projection to expand the original remaining attempt budget.
    pub(crate) fn validate_projection(&self, assignment: &Assignment) -> Result<(), Error> {
        let mut expected = self.program.assignment.clone();
        if expected.goals.len() != assignment.goals.len() {
            return Err(Error::ConfigurationMismatch);
        }
        for (baseline, projected) in expected.goals.iter_mut().zip(&assignment.goals) {
            if projected.attempts_remaining > baseline.attempts_remaining
                || projected.accepted < baseline.accepted
            {
                return Err(Error::ConfigurationMismatch);
            }
            baseline.accepted = projected.accepted;
            baseline.pending = projected.pending;
            baseline.attempts_remaining = projected.attempts_remaining;
        }
        if &expected != assignment {
            return Err(Error::ConfigurationMismatch);
        }
        Ok(())
    }
}

/// Validate an equipment snapshot before enrollment, without inventing a goal.
/// Program binding still validates every target, recipe and assignment separately.
pub fn validate_configuration(config: &Configuration) -> Result<(), Error> {
    if !valid_id(&config.id)
        || !valid_id(&config.rig_id)
        || !valid_id(&config.camera_id)
        || config
            .filter_wheel_id
            .as_ref()
            .is_some_and(|id| !valid_id(id))
        || config.filters.is_empty()
        || config.filters.len() > MAX_CAPABILITIES
        || config.binning_modes.is_empty()
        || config.binning_modes.len() > MAX_CAPABILITIES
        || config.readout_modes.is_empty()
        || config.readout_modes.len() > MAX_CAPABILITIES
        || config.exposure_min_ms == 0
        || config.exposure_min_ms > config.exposure_max_ms
        || !config.gain.validate()
        || !config.offset.validate()
    {
        return Err(Error::InvalidConfiguration);
    }
    let mut filters = BTreeSet::new();
    let mut positions = BTreeSet::new();
    for filter in &config.filters {
        if !valid_id(&filter.id)
            || !filters.insert(&filter.id)
            || match (&config.filter_wheel_id, filter.position) {
                (Some(_), Some(position)) => position < 0 || !positions.insert(position),
                (None, None) => config.filters.len() != 1,
                _ => true,
            }
        {
            return Err(Error::InvalidConfiguration);
        }
    }
    if config
        .binning_modes
        .iter()
        .any(|mode| mode.x < 1 || mode.y < 1)
        || config.binning_modes.iter().collect::<BTreeSet<_>>().len() != config.binning_modes.len()
        || config.readout_modes.iter().any(|mode| *mode < 0)
        || config.readout_modes.iter().collect::<BTreeSet<_>>().len() != config.readout_modes.len()
    {
        return Err(Error::InvalidConfiguration);
    }
    Ok(())
}

/// Validate framing without inventing an allocated goal or current conditions.
pub fn validate_target(target: &Target) -> Result<(), Error> {
    if !valid_id(&target.id)
        || target.name.trim().is_empty()
        || target.name.len() > 256
        || target.name.chars().any(char::is_control)
        || target.icrs_ra_mas >= FULL_CIRCLE
        || !(-POLE..=POLE).contains(&target.icrs_dec_mas)
        || target
            .position_angle_mas
            .is_some_and(|angle| angle >= FULL_CIRCLE)
    {
        return Err(Error::InvalidTarget);
    }
    Ok(())
}

/// Validate a concrete recipe against an already validated configuration.
pub fn validate_recipe(recipe: &Recipe, config: &Configuration) -> Result<(), Error> {
    validate_recipe_shape(recipe)?;
    if !config
        .filters
        .iter()
        .any(|filter| filter.id == recipe.filter_id)
        || recipe.exposure_ms < config.exposure_min_ms
        || recipe.exposure_ms > config.exposure_max_ms
        || !config.binning_modes.contains(&recipe.binning)
        || !config.readout_modes.contains(&recipe.readout_mode)
        || !config.gain.accepts(recipe.gain)
        || !config.offset.accepts(recipe.offset)
    {
        return Err(Error::InvalidRecipe);
    }
    Ok(())
}

pub(crate) fn validate_recipe_shape(recipe: &Recipe) -> Result<(), Error> {
    if !valid_id(&recipe.id)
        || !valid_id(&recipe.filter_id)
        || recipe.exposure_ms == 0
        || recipe.binning.x < 1
        || recipe.binning.y < 1
        || recipe.readout_mode < 0
        || recipe.gain.is_some_and(|value| value < 0)
        || recipe.offset.is_some_and(|value| value < 0)
    {
        return Err(Error::InvalidRecipe);
    }
    Ok(())
}
