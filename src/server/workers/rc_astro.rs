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
    /// The license line the tool printed, which gives the licensee. It is
    /// not sent to a browser: an apply that fails for want of a license
    /// shows it to the person who applied.
    #[serde(skip)]
    license: Option<String>,
}

impl ProviderState {
    /// The schema a step runs with, or why it cannot run.
    fn runnable(&self) -> Result<ExternalToolSchema, String> {
        match (&self.schema, self.state) {
            (Some(schema), ProviderReadiness::Ready) => Ok(schema.clone()),
            _ => Err(format!(
                "{}{}",
                self.reason
                    .clone()
                    .unwrap_or_else(|| format!("{} cannot run", self.name)),
                self.license
                    .as_deref()
                    .map(|line| format!(" ({line})"))
                    .unwrap_or_default()
            )),
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
                "{} is not licensed {}",
                schema.name,
                if *local {
                    "on this server".to_string()
                } else {
                    format!("on {worker}")
                }
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
        license: chosen
            .filter(|(_, _, schema)| !schema.licensed)
            .and_then(|(_, _, schema)| schema.license_message.clone()),
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
        // The job that waits holds its leased workers for the step.
        payload: runner.payload(schema, request, headers, person, steps::held_threads())?,
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

/// How long the server waits for a worker to answer a license check: the
/// check's own limit, and time for the worker to claim the task.
const WORKER_LICENSE_LIMIT: std::time::Duration = std::time::Duration::from_secs(
    crate::remote_work::rc_astro::LICENSE_CHECK_LIMIT.as_secs() + 30,
);

impl Workers {
    /// RC-Astro's account status on this server, checked online now. Runs
    /// `rc-astro license --json` once, so call it only when a person asks.
    /// Blocks for as long as rc-astro takes, up to its limit.
    pub fn check_local_license(
        &self,
    ) -> Result<crate::remote_work::rc_astro::LicenseCheck, String> {
        let cli = crate::remote_work::rc_astro::locate(None).ok_or_else(|| {
            format!(
                "rc-astro was not found on this server (looked in {})",
                crate::remote_work::rc_astro::searched_places()
            )
        })?;
        Ok(crate::remote_work::rc_astro::check_license(
            cli.executable(),
            crate::remote_work::rc_astro::LICENSE_CHECK_LIMIT,
        ))
    }

    /// RC-Astro's account status on one connected worker, checked online
    /// now by that worker, as the account it runs under. Blocks until the
    /// worker answers.
    pub fn check_worker_license(
        &self,
        session: &str,
    ) -> Result<crate::remote_work::rc_astro::LicenseCheck, String> {
        let (name, local) = self.board.license_checkable(session)?;
        if local {
            return self.check_local_license();
        }
        let ticket = self.board.ticket()?;
        ticket.submit(NewTask {
            kind: TaskKind::RcAstroLicense,
            label: format!("RC-Astro license check on {name}"),
            person: true,
            needs: TaskNeeds::LicenseCheck {
                session: session.to_string(),
            },
            spec: serde_json::json!({}),
            inputs: Vec::new(),
            // A task has an output folder; the report is its result.
            outputs: vec![("report.json".into(), false)],
        })?;
        let began = std::time::Instant::now();
        let finished = ticket
            .wait(&|| began.elapsed() >= WORKER_LICENSE_LIMIT, &mut |_, _| {})
            .map_err(|error| match error {
                WaitError::Stopped => format!("{name} did not answer the license check"),
                WaitError::Failed(message) => message,
            })?;
        serde_json::from_value(finished.result)
            .map_err(|error| format!("{name} sent back a report that could not be read: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(licensed: bool, message: Option<&str>) -> StepOffer {
        StepOffer::of(
            serde_json::from_value(serde_json::json!({
                "schema_version": 6, "cli_version": "2.6.6", "key": "nxt",
                "name": "RC-Astro NoiseXTerminator", "ml_version": 3, "licensed": licensed,
                "license_message": message, "parameters": [],
            }))
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_browser_is_not_sent_the_license_line_but_a_failed_apply_gives_it() {
        let nxt = steps::provider("nxt").unwrap();
        let offers = vec![(
            "studio".to_string(),
            false,
            vec![offer(false, Some("Trial of Ann Example expired"))],
        )];
        let state = provider_state(nxt, &offers);
        assert_eq!(state.state, ProviderReadiness::NotLicensed);
        assert_eq!(state.remedy, Some(Remedy::License));
        assert_eq!(
            state.reason.as_deref(),
            Some("RC-Astro NoiseXTerminator is not licensed on studio")
        );
        // The handler clears the schema's own copy; nothing else carries it.
        let mut sent = state.clone();
        sent.schema.as_mut().unwrap().license_message = None;
        let json = serde_json::to_string(&sent).unwrap();
        assert!(!json.contains("Ann Example"), "{json}");
        // The person who applies the step is told.
        let refused = state.runnable().unwrap_err();
        assert!(
            refused.contains("Trial of Ann Example expired"),
            "{refused}"
        );
    }

    #[test]
    fn a_provider_no_machine_has_reports_what_to_do_and_one_that_is_ready_reports_nothing() {
        let nxt = steps::provider("nxt").unwrap();
        let missing = provider_state(nxt, &[]);
        assert_eq!(missing.state, ProviderReadiness::NotInstalled);
        assert_eq!(missing.remedy, Some(Remedy::InstallRcAstro));
        assert_eq!(missing.reason.as_deref(), Some(NOT_INSTALLED));
        let model = steps::provider(crate::remote_work::noise_model::TOOL).unwrap();
        assert_eq!(
            provider_state(model, &[]).remedy,
            Some(Remedy::DownloadNoiseModel)
        );
        // A licensed copy on this server wins over an unlicensed one on a
        // worker, and the provider is ready with no reason.
        let ready = provider_state(
            nxt,
            &[
                ("studio".to_string(), false, vec![offer(false, None)]),
                ("This server".to_string(), true, vec![offer(true, None)]),
            ],
        );
        assert_eq!(ready.state, ProviderReadiness::Ready);
        assert_eq!(ready.reason, None);
        assert!(ready.this_server);
        assert_eq!(ready.workers, ["studio"]);
    }
}
