//! The processing steps on workers: which worker's RC-Astro tools and
//! noise model a chain uses, and one step run as a task on the board.

use seiza_stacking::{ExternalToolRequest, ExternalToolSchema, LinearImage, ProcessedStackImage};

use super::board::{NewTask, TaskNeeds, WaitError};
use super::Workers;
use crate::remote_work::noise_model;
use crate::remote_work::protocol::{HeaderCard, TaskKind};
use crate::remote_work::rc_astro::{self as task, StepResult, StepSpec};

/// The message when no worker has rc-astro.
pub const NOT_INSTALLED: &str = "rc-astro is not installed on this server or on a connected worker";

/// The message when no worker has the noise model, with what to do.
pub const NOISE_MODEL_NOT_INSTALLED: &str =
    "The noise model is not installed on this server or on a connected worker. \
     Open Settings, choose Reference catalogs, and click Download under Noise model";

/// Where the processing steps run, and at what priority.
#[derive(Clone, Copy)]
pub struct RcAstroRunner<'a> {
    pub workers: &'a Workers,
    /// A person waits for the result, so its tasks go ahead of automatic
    /// work.
    pub person: bool,
}

/// The tools the workers offer, as `GET /api/tools/rc-astro` lists them.
pub struct Offered {
    /// For each tool, the schema the next run would use.
    pub tools: Vec<ExternalToolSchema>,
    /// Whether this server has rc-astro.
    pub this_server: bool,
    /// The connected workers that have rc-astro, by name.
    pub workers: Vec<String>,
    /// For each tool, the connected workers that take it, by name.
    pub tool_workers: std::collections::BTreeMap<String, Vec<String>>,
}

impl Workers {
    /// Whether this server would run a step with `schema` itself, were it
    /// queued now. See [`super::board::Board::runs_here`].
    pub fn rc_astro_runs_here(&self, schema: &ExternalToolSchema) -> bool {
        self.refresh_local();
        let (kind, needs) = TaskNeeds::step(schema);
        self.board.runs_here(kind, &needs)
    }

    /// The noise model the next run would use and who offers it: the first
    /// connected worker's by name, otherwise this server's. Opens ONNX
    /// Runtime on this server when its library is installed and the offers
    /// are a minute old.
    pub fn noise_model_offers(&self) -> Vec<(String, bool, noise_model::Identity)> {
        self.refresh_local();
        self.board.noise_model_offers()
    }

    /// The schema each tool would run with: the first connected worker's by
    /// name that takes the tool, otherwise this server's. A licensed schema
    /// wins over an unlicensed one. Spawns short-lived processes when this
    /// server's tools were last probed a minute ago.
    pub fn rc_astro_tools(&self) -> Offered {
        self.refresh_local();
        let offers = self.board.rc_astro_offers();
        let mut tools = Vec::new();
        for tool in task::TOOLS {
            let mut candidates = offers
                .iter()
                .flat_map(|(_, _, schemas)| schemas.iter().filter(|schema| schema.key == tool));
            let licensed = offers
                .iter()
                .flat_map(|(_, _, schemas)| schemas.iter())
                .find(|schema| schema.key == tool && schema.licensed);
            if let Some(schema) = licensed.or_else(|| candidates.next()) {
                tools.push(schema.clone());
            }
        }
        Offered {
            tools,
            this_server: offers.iter().any(|(_, local, _)| *local),
            workers: offers
                .iter()
                .filter(|(_, local, _)| !local)
                .map(|(name, _, _)| name.clone())
                .collect(),
            tool_workers: task::TOOLS
                .iter()
                .map(|tool| {
                    let names = offers
                        .iter()
                        .filter(|(_, local, schemas)| {
                            !local && schemas.iter().any(|schema| schema.key == *tool)
                        })
                        .map(|(name, _, _)| name.clone())
                        .collect();
                    (tool.to_string(), names)
                })
                .collect(),
        }
    }

    /// The schema `tool` runs with, or why it cannot run.
    pub fn rc_astro_schema(&self, tool: &str) -> Result<ExternalToolSchema, String> {
        if noise_model::is_step(tool) {
            return self
                .noise_model_offers()
                .first()
                .map(|(_, _, identity)| identity.schema())
                .ok_or_else(|| NOISE_MODEL_NOT_INSTALLED.to_string());
        }
        self.refresh_local();
        let offers = self.board.rc_astro_offers();
        let mut unlicensed = None;
        for (worker, local, schemas) in &offers {
            if let Some(schema) = schemas.iter().find(|schema| schema.key == tool) {
                if schema.licensed {
                    return Ok(schema.clone());
                }
                unlicensed.get_or_insert((worker, *local, schema));
            }
        }
        match unlicensed {
            Some((worker, local, schema)) => Err(format!(
                "{} is not licensed {}{}",
                schema.name,
                if local {
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
            None => Err(NOT_INSTALLED.into()),
        }
    }
}

impl RcAstroRunner<'_> {
    /// Run one tool over `image` on the first worker that offers it at
    /// `schema`'s version. `cancel` takes the task off the board, and the
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
    let ticket = board.ticket()?;
    let input = ticket.dir().join(task::INPUT);
    task::write_image(&input, image)?;
    let (kind, needs) = TaskNeeds::step(schema);
    let (spec, outputs) = if kind == TaskKind::NoiseModelStep {
        let identity = board
            .noise_model_identity(&schema.cli_version)
            .ok_or_else(|| NOISE_MODEL_NOT_INSTALLED.to_string())?;
        let spec = noise_model::StepSpec {
            identity,
            amounts: noise_model::Amounts::from_request(request)?,
            person,
        };
        (
            serde_json::to_value(&spec).map_err(|error| error.to_string())?,
            vec![(task::PROCESSED.into(), true)],
        )
    } else {
        let spec = StepSpec {
            schema: schema.clone(),
            request: request.clone(),
            headers: HeaderCard::from_fits(headers),
        };
        (
            serde_json::to_value(&spec).map_err(|error| error.to_string())?,
            vec![(task::PROCESSED.into(), true), (task::STARS.into(), false)],
        )
    };
    ticket.submit(NewTask {
        kind,
        label: format!(
            "{} on a {}×{} image",
            schema.name, image.width, image.height
        ),
        person,
        needs,
        spec,
        inputs: vec![(task::INPUT.into(), input)],
        outputs,
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
