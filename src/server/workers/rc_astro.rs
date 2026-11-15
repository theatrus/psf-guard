//! Processing steps on workers: which worker's provider a chain uses for
//! each step, and one step run as a task on the board. Every provider goes
//! through here, whatever its engine: an RC-Astro tool or a Seiza model.

use seiza_stacking::{ExternalToolRequest, ExternalToolSchema, LinearImage, ProcessedStackImage};
use serde::Serialize;

use super::board::{NewTask, TaskNeeds, WaitError};
use super::Workers;
use crate::remote_work::protocol::TaskKind;
use crate::remote_work::rc_astro::{self as task, StepResult};
use crate::remote_work::steps::{self, Category, Remedy, StepOffer};

pub use crate::remote_work::noise_model::NOT_INSTALLED as NOISE_MODEL_NOT_INSTALLED;
pub use crate::remote_work::rc_astro::NOT_INSTALLED;

/// Where processing steps run, and at what priority.
#[derive(Clone, Copy)]
pub struct RcAstroRunner<'a> {
    pub workers: &'a Workers,
    /// A person waits for the result, so its tasks go ahead of automatic
    /// work.
    pub person: bool,
}

/// Whether a provider can run a step now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReadiness {
    Ready,
    /// A machine has it, but none has a license for it.
    NotLicensed,
    /// No machine the placement allows has it.
    NotInstalled,
}

/// One provider's state, as `GET /api/tools/processing` lists it.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderState {
    pub category: Category,
    pub provider: String,
    pub name: String,
    pub state: ProviderReadiness,
    /// Why it cannot run, with what a person does about it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The kind of action that fixes it, for the controls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remedy: Option<Remedy>,
    /// The version the next run would use, for a person to read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The schema the next run would use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<ExternalToolSchema>,
    /// Whether this server has it.
    pub this_server: bool,
    /// The connected workers that have it, by name.
    pub workers: Vec<String>,
}

impl ProviderState {
    /// The schema a step runs with, or why it cannot run.
    fn runnable(&self) -> Result<ExternalToolSchema, String> {
        match (&self.schema, self.state) {
            (Some(schema), ProviderReadiness::Ready) => Ok(schema.clone()),
            _ => Err(self
                .reason
                .clone()
                .unwrap_or_else(|| format!("{} cannot run", self.name))),
        }
    }
}

/// One provider's state from the offers of the machines that may run it:
/// other machines' first, by name, then this server's. A licensed schema
/// wins over an unlicensed one.
fn provider_state(
    provider: &steps::Provider,
    offers: &[(String, bool, Vec<StepOffer>)],
) -> ProviderState {
    let having: Vec<(&String, bool, &ExternalToolSchema)> = offers
        .iter()
        .filter_map(|(name, local, steps)| {
            let offer = steps.iter().find(|offer| offer.provider == provider.key)?;
            Some((name, *local, &offer.schema))
        })
        .collect();
    let chosen = having
        .iter()
        .find(|(_, _, schema)| schema.licensed)
        .or(having.first());
    let (missing, remedy) = steps::runner(provider.engine).missing();
    let (state, reason, remedy) = match chosen {
        Some((_, _, schema)) if schema.licensed => (ProviderReadiness::Ready, None, None),
        Some((worker, local, schema)) => (
            ProviderReadiness::NotLicensed,
            Some(format!(
                "{} is not licensed {}{}",
                schema.name,
                if *local {
                    "on this server".to_string()
                } else {
                    format!("on {worker}")
                },
                schema
                    .license_message
                    .as_deref()
                    .map(|message| format!(" ({message})"))
                    .unwrap_or_default()
            )),
            Some(Remedy::License),
        ),
        None => (
            ProviderReadiness::NotInstalled,
            Some(missing.to_string()),
            Some(remedy),
        ),
    };
    ProviderState {
        category: provider.category,
        provider: provider.key.into(),
        name: chosen
            .map(|(_, _, schema)| schema.name.clone())
            .unwrap_or_else(|| provider.name.into()),
        state,
        reason,
        remedy,
        version: chosen.map(|(_, _, schema)| {
            crate::remote_work::noise_model::display_version(&schema.cli_version).to_string()
        }),
        schema: chosen.map(|(_, _, schema)| (*schema).clone()),
        this_server: having.iter().any(|(_, local, _)| *local),
        workers: having
            .iter()
            .filter(|(_, local, _)| !local)
            .map(|(name, _, _)| (*name).clone())
            .collect(),
    }
}

impl Workers {
    /// Whether this server would run a step with `schema` itself, were it
    /// queued now. See [`super::board::Board::runs_here`].
    pub fn step_runs_here(&self, schema: &ExternalToolSchema) -> bool {
        self.refresh_local();
        self.board
            .runs_here(TaskKind::ProcessingStep, &TaskNeeds::step(schema))
    }

    /// Every provider's state, in the order a chain runs their categories.
    /// Probes this server's own when the last probe is a minute old: that
    /// spawns short-lived rc-astro processes, and opens ONNX Runtime when
    /// its library is installed.
    pub fn providers(&self) -> Vec<ProviderState> {
        self.refresh_local();
        let offers = self.board.step_offers();
        steps::PROVIDERS
            .iter()
            .map(|provider| provider_state(provider, &offers))
            .collect()
    }

    /// The schema a provider's step runs with, or why it cannot run.
    pub fn step_schema(&self, provider: &str) -> Result<ExternalToolSchema, String> {
        let known = steps::provider(provider)
            .ok_or_else(|| format!("unknown processing step {provider:?}"))?;
        self.refresh_local();
        provider_state(known, &self.board.step_offers()).runnable()
    }
}

impl RcAstroRunner<'_> {
    /// Run one step over `image` on the first worker that offers its
    /// provider at `schema`'s version. `cancel` takes the task off the board, and the
    /// worker running it stops. `progress` is told how much is done and
    /// which worker runs it: `None` for this server.
    pub fn run(
        &self,
        schema: &ExternalToolSchema,
        request: &ExternalToolRequest,
        image: &LinearImage,
        headers: &[(String, seiza_fits::HeaderValue)],
        cancel: Option<&seiza_stacking::CancelSignal>,
        progress: &mut dyn FnMut(f32, Option<&str>),
    ) -> Result<ProcessedStackImage, String> {
        self.workers.refresh_local();
        run_on_board(
            &self.workers.board,
            self.person,
            schema,
            request,
            image,
            headers,
            cancel,
            progress,
        )
    }
}

/// Run one tool over `image` as a task on `board`, as [`RcAstroRunner::run`]
/// does, without starting this server's own worker.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_on_board(
    board: &std::sync::Arc<super::Board>,
    person: bool,
    schema: &ExternalToolSchema,
    request: &ExternalToolRequest,
    image: &LinearImage,
    headers: &[(String, seiza_fits::HeaderValue)],
    cancel: Option<&seiza_stacking::CancelSignal>,
    progress: &mut dyn FnMut(f32, Option<&str>),
) -> Result<ProcessedStackImage, String> {
    let provider = steps::provider(&schema.key)
        .ok_or_else(|| format!("unknown processing step {:?}", schema.key))?;
    let runner = steps::runner(provider.engine);
    let ticket = board.ticket()?;
    let input = ticket.dir().join(task::INPUT);
    task::write_image(&input, image)?;
    let spec = steps::StepSpec {
        category: provider.category,
        provider: provider.key.into(),
        payload: runner.payload(schema, request, headers, person)?,
    };
    ticket.submit(NewTask {
        kind: TaskKind::ProcessingStep,
        label: format!(
            "{} on a {}×{} image",
            schema.name, image.width, image.height
        ),
        person,
        needs: TaskNeeds::step(schema),
        spec: serde_json::to_value(&spec).map_err(|error| error.to_string())?,
        inputs: vec![(task::INPUT.into(), input)],
        outputs: runner.outputs(),
    })?;
    let stopped = || cancel.is_some_and(|cancel| cancel.is_cancelled());
    let finished = ticket
        .wait(&stopped, progress)
        .map_err(|error| match error {
            WaitError::Stopped => format!("{} was stopped", schema.name),
            WaitError::Failed(message) => message,
        })?;
    let result: StepResult = serde_json::from_value(finished.result).map_err(|error| {
        format!(
            "{} sent back a result that could not be read: {error}",
            finished.worker
        )
    })?;
    let processed = finished
        .outputs
        .get(task::PROCESSED)
        .ok_or_else(|| format!("{} sent back no processed image", finished.worker))?;
    Ok(ProcessedStackImage {
        image: task::read_image(processed)?,
        stars: finished
            .outputs
            .get(task::STARS)
            .map(|path| task::read_image(path))
            .transpose()?,
        device: result.device,
        warnings: result.warnings,
    })
}
