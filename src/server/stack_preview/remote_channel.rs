//! A stack build's channel as a worker task (`stack_channel`).
//!
//! This server prepares the channel as it does for itself: the reference,
//! the calibration masters, the resume decision and the orientation. The
//! task then sends the frames, the master files and the checkpoint to a
//! worker, which runs the same integration ([`integrate::integrate_channel`])
//! and reports its events. This server applies them to the build's card,
//! puts the stack, the clipped map and the checkpoint where it keeps them,
//! and renders the previews.
//!
//! This server stacks its own builds in process, so their running image
//! shows on the card; only a worker on another machine claims this task.

use std::collections::BTreeMap;

use super::integrate::{self, ChannelEvents, RestoredFrames};
use super::*;
use crate::remote_work::protocol::TaskKind;
use crate::remote_work::TaskReport;
use crate::server::workers::{NewTask, TaskNeeds, WaitError, Waiting};

const STACK: &str = "stack.fits";
const CLIPPED: &str = "clipped.fits";
const CHECKPOINT_CONTEXT: &str = "checkpoint.ctx";
const CHECKPOINT_MANIFEST: &str = "checkpoint.json";

/// How long a stopped channel has to checkpoint what it integrated and send
/// it, before this server stops waiting.
const FINISH_ON_STOP: std::time::Duration = std::time::Duration::from_secs(120);

/// A WCS, as a spec sends it.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WireWcs {
    crval: (f64, f64),
    crpix: (f64, f64),
    cd: [[f64; 2]; 2],
    sip: Option<WireSip>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WireSip {
    order: u8,
    a: Vec<f64>,
    b: Vec<f64>,
    ap: Vec<f64>,
    bp: Vec<f64>,
}

impl From<&seiza::Wcs> for WireWcs {
    fn from(wcs: &seiza::Wcs) -> Self {
        Self {
            crval: wcs.crval,
            crpix: wcs.crpix,
            cd: wcs.cd,
            sip: wcs.sip.as_ref().map(|sip| WireSip {
                order: sip.order,
                a: sip.a.clone(),
                b: sip.b.clone(),
                ap: sip.ap.clone(),
                bp: sip.bp.clone(),
            }),
        }
    }
}

impl From<WireWcs> for seiza::Wcs {
    fn from(wcs: WireWcs) -> Self {
        Self {
            crval: wcs.crval,
            crpix: wcs.crpix,
            cd: wcs.cd,
            sip: wcs.sip.map(|sip| seiza::Sip {
                order: sip.order,
                a: sip.a,
                b: sip.b,
                ap: sip.ap,
                bp: sip.bp,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WireOrientation {
    NorthUp {
        wcs: WireWcs,
        source: String,
    },
    Reference {
        /// Whether the reference faces north up, and what decided it.
        anchor: Option<(bool, String)>,
    },
}

/// What decided a half turn, as the constant the orientation record
/// stores. A source this release does not know falls back to the exposure
/// vote, with a warning.
fn orientation_source(source: &str) -> &'static str {
    [
        orientation_source::SKY_ANCHOR,
        orientation_source::PIER_SIDE,
        orientation_source::EXPOSURE_MAJORITY,
    ]
    .into_iter()
    .find(|known| *known == source)
    .unwrap_or_else(|| {
        tracing::warn!("Unknown orientation source {source:?}; recording the exposure vote");
        orientation_source::EXPOSURE_MAJORITY
    })
}

impl WireOrientation {
    fn from_orientation(orientation: &integrate::Orientation) -> Self {
        match orientation {
            integrate::Orientation::NorthUp { wcs, source } => Self::NorthUp {
                wcs: wcs.into(),
                source: source.clone(),
            },
            integrate::Orientation::Reference { anchor } => Self::Reference {
                anchor: anchor.map(|(north_up, source)| (north_up, source.to_string())),
            },
        }
    }

    fn into_orientation(self) -> integrate::Orientation {
        match self {
            Self::NorthUp { wcs, source } => integrate::Orientation::NorthUp {
                wcs: wcs.into(),
                source,
            },
            Self::Reference { anchor } => integrate::Orientation::Reference {
                anchor: anchor.map(|(north_up, source)| (north_up, orientation_source(&source))),
            },
        }
    }
}

/// One calibration session, with its master files named by input.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SessionSpec {
    recipe: crate::calibration::MasterRecipe,
    applied: crate::calibration::AppliedCalibration,
    match_tolerances: seiza_calibration::MatchTolerances,
}

/// Everything a worker needs to integrate a channel. The frames' paths and
/// the master recipes' files hold the names of the task's inputs.
#[derive(Clone, Serialize, Deserialize)]
struct ChannelSpec {
    job_id: String,
    group: PreparedGroup,
    assignments: Vec<usize>,
    sessions: Vec<SessionSpec>,
    applied: crate::calibration::AppliedCalibration,
    /// The checkpoint's ledger; its context is the input
    /// [`CHECKPOINT_CONTEXT`].
    checkpoint: Option<resume::ResumeManifest>,
    accepted_only: bool,
    scoring: StackScoringSettings,
    method: StackMethod,
    gpu: StackGpu,
    order: snr::StackFrameOrder,
    calibration_fingerprint: String,
    target_id: i32,
    filter_name: String,
    exposure_group_key: Option<String>,
    orientation: WireOrientation,
}

/// What a worker's integration reports, one per event.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum ChannelEvent {
    CalibrationNote {
        note: String,
    },
    ResumeNote {
        note: String,
    },
    CheckpointUnusable {
        why: String,
    },
    FramesRestored {
        restored: Box<RestoredFrames>,
    },
    Frame {
        decision: Box<StackFrameDecision>,
        exposure_seconds: f64,
    },
    Snr {
        progressive: Box<snr::ProgressiveSnr>,
    },
    Phase {
        phase: String,
        final_pass: Option<FinalPassProgress>,
    },
    IntegratedFractions {
        fractions: Vec<f32>,
    },
    SkyOrientation {
        orientation: Box<StackSkyOrientation>,
    },
}

impl ChannelEvent {
    /// Hand the event to `events`, as the integration did on the worker.
    fn apply(self, events: &mut dyn ChannelEvents) {
        match self {
            Self::CalibrationNote { note } => events.calibration_note(note),
            Self::ResumeNote { note } => events.resume_note(&note),
            Self::CheckpointUnusable { why } => events.checkpoint_unusable(&why),
            Self::FramesRestored { restored } => events.frames_restored(*restored),
            Self::Frame {
                decision,
                exposure_seconds,
            } => events.frame(*decision, exposure_seconds),
            Self::Snr { progressive } => events.snr(*progressive),
            Self::Phase { phase, final_pass } => events.phase(phase, final_pass),
            Self::IntegratedFractions { fractions } => events.integrated_fractions(fractions),
            Self::SkyOrientation { orientation } => events.sky_orientation(*orientation),
        }
    }
}

/// What a worker's channel made.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChannelResult {
    /// False when the channel stopped before it finished; the checkpoint,
    /// when it wrote one, keeps the frames it integrated.
    built: bool,
    progressive: Option<snr::ProgressiveSnr>,
}

/// The integration's events, sent to the server through the task.
struct WorkerEvents<'a> {
    report: &'a mut dyn TaskReport,
    frames: usize,
    total: usize,
}

impl WorkerEvents<'_> {
    fn send(&mut self, event: ChannelEvent) {
        match serde_json::to_value(event) {
            Ok(event) => self.report.event(event),
            Err(error) => tracing::warn!("A stack event could not be written: {error}"),
        }
    }
}

impl WorkerEvents<'_> {
    /// The live pass is most of a channel's work, the final pass the rest.
    const LIVE_SHARE: f32 = 0.8;
}

impl ChannelEvents for WorkerEvents<'_> {
    fn calibration_note(&mut self, note: String) {
        self.send(ChannelEvent::CalibrationNote { note });
    }

    fn resume_note(&mut self, note: &str) {
        self.send(ChannelEvent::ResumeNote { note: note.into() });
    }

    fn checkpoint_unusable(&mut self, why: &str) {
        self.send(ChannelEvent::CheckpointUnusable { why: why.into() });
    }

    fn frames_restored(&mut self, restored: RestoredFrames) {
        self.frames = restored.frames.len();
        self.send(ChannelEvent::FramesRestored {
            restored: Box::new(restored),
        });
    }

    fn frame(&mut self, decision: StackFrameDecision, exposure_seconds: f64) {
        self.frames += 1;
        self.report
            .fraction(Self::LIVE_SHARE * self.frames as f32 / self.total.max(1) as f32);
        self.send(ChannelEvent::Frame {
            decision: Box::new(decision),
            exposure_seconds,
        });
    }

    fn snr(&mut self, progressive: snr::ProgressiveSnr) {
        self.send(ChannelEvent::Snr {
            progressive: Box::new(progressive),
        });
    }

    fn phase(&mut self, phase: String, final_pass: Option<FinalPassProgress>) {
        if let Some(pass) = &final_pass {
            let done = ((pass.pass - 1) as f32 + pass.frame as f32 / pass.frames.max(1) as f32)
                / pass.passes.max(1) as f32;
            self.report
                .fraction(Self::LIVE_SHARE + (1.0 - Self::LIVE_SHARE) * done);
        }
        self.send(ChannelEvent::Phase { phase, final_pass });
    }

    fn integrated_fractions(&mut self, fractions: Vec<f32>) {
        self.send(ChannelEvent::IntegratedFractions { fractions });
    }

    fn sky_orientation(&mut self, orientation: StackSkyOrientation) {
        self.send(ChannelEvent::SkyOrientation {
            orientation: Box::new(orientation),
        });
    }

    /// The running image shows only for a build this server stacks.
    fn live(&mut self, _: &seiza_stacking::LiveStacker, _: u32, _: bool) {}

    /// No running image to keep a cadence for: each session pushes as one
    /// batch, between the SNR curve's depths.
    fn batch_frames(&self) -> usize {
        usize::MAX
    }

    fn observe_batch(&mut self, _: std::time::Duration, _: usize) {}
}

/// Run a channel on this worker. `inputs` gives where each input is; the
/// stack, the clipped map and the checkpoint are written into `output_dir`,
/// and the final pass keeps its copies of the frames in `work_dir`. The
/// channel takes a `share` of the worker's cores and memory: one of the
/// tasks it may run at once.
pub(crate) fn execute(
    spec: serde_json::Value,
    inputs: &BTreeMap<String, PathBuf>,
    output_dir: &FsPath,
    work_dir: &FsPath,
    share: usize,
    stop: &Stop,
    report: &mut dyn TaskReport,
) -> Result<serde_json::Value, String> {
    let spec: ChannelSpec = serde_json::from_value(spec)
        .map_err(|error| format!("The stack channel could not be read: {error}"))?;
    let input = |name: &FsPath| -> Result<PathBuf, String> {
        let name = name.to_string_lossy();
        inputs
            .get(name.as_ref())
            .cloned()
            .ok_or_else(|| format!("The stack channel has no input called {name}"))
    };
    let mut group = spec.group;
    for frame in &mut group.frames {
        frame.path = input(&frame.path)?;
        frame.read_fingerprint = Some(source_fingerprint(&frame.path));
    }
    let mut sessions = Vec::with_capacity(spec.sessions.len());
    for session in spec.sessions {
        let mut missing = None;
        let recipe = session.recipe.with_files(|name| {
            input(name).unwrap_or_else(|problem| {
                missing = Some(problem);
                name.to_path_buf()
            })
        });
        if let Some(problem) = missing {
            return Err(problem);
        }
        let masters = recipe
            .load(|path| path.to_path_buf())
            .map_err(|error| format!("{error:#}"))?;
        sessions.push(crate::calibration::CalibrationSession {
            masters,
            recipe,
            applied: session.applied,
            match_tolerances: session.match_tolerances,
        });
    }
    let plan = crate::calibration::CalibrationPlan {
        assignments: spec.assignments,
        sessions,
        applied: spec.applied,
    };
    let checkpoint = match (spec.checkpoint, inputs.get(CHECKPOINT_CONTEXT)) {
        (Some(manifest), Some(context_path)) => Some(resume::ResumeState {
            context_path: context_path.clone(),
            manifest,
        }),
        _ => None,
    };
    let share = share.max(1);
    let threads = (crate::concurrency::logical_cores() / share).max(1);
    let pool = crate::concurrency::ComputePool::take("stack-worker", threads)
        .map_err(|error| error.to_string())?;
    let scratch = work_dir.join("final-pass");
    let policy = crate::concurrency::WorkerPolicy::default();
    let total = group.frames.len();
    let mut events = WorkerEvents {
        report,
        frames: 0,
        total,
    };
    let integrated = integrate::integrate_channel(
        integrate::ChannelWork {
            job_id: &spec.job_id,
            group: &group,
            plan: &plan,
            checkpoint,
            accepted_only: spec.accepted_only,
            scoring: spec.scoring,
            method: spec.method,
            gpu: spec.gpu,
            order: spec.order,
            calibration_fingerprint: &spec.calibration_fingerprint,
            identity: integrate::CheckpointIdentity {
                target_id: spec.target_id,
                filter_name: spec.filter_name,
                exposure_group_key: spec.exposure_group_key,
            },
            orientation: spec.orientation.into_orientation(),
            outputs: integrate::ChannelOutputs {
                fits: output_dir.join(STACK),
                clipped: output_dir.join(CLIPPED),
                checkpoint_context: output_dir.join(CHECKPOINT_CONTEXT),
                checkpoint_manifest: output_dir.join(CHECKPOINT_MANIFEST),
                scratch: scratch.clone(),
            },
            pool: &pool,
            threads: execution::ThreadBudget::from_total(threads),
            available_memory: crate::concurrency::available_memory_bytes()
                .map(|bytes| bytes / share as u64),
            worker_policy: &policy,
            cancel: stop,
        },
        &mut events,
    );
    let _ = std::fs::remove_dir_all(&scratch);
    let result = match integrated? {
        integrate::ChannelOutcome::Built(built) => ChannelResult {
            built: true,
            progressive: Some(built.progressive),
        },
        integrate::ChannelOutcome::Cancelled => ChannelResult {
            built: false,
            progressive: None,
        },
    };
    serde_json::to_value(result).map_err(|error| error.to_string())
}

/// The needs of a channel task: a worker that stacks as this server does.
fn needs() -> TaskNeeds {
    TaskNeeds::Stack {
        stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
        release: env!("CARGO_PKG_VERSION").into(),
    }
}

/// Whether a worker on another machine stacks this channel, by the
/// placement in Settings › Workers: always under workers only, never under
/// this server only, and under workers first when such a worker is
/// connected.
pub(super) fn wanted(state: &AppState) -> bool {
    use crate::server::workers::board::WorkPlacement;
    match state.workers.board.placement(TaskKind::StackChannel) {
        WorkPlacement::ThisServer => false,
        WorkPlacement::WorkersOnly => true,
        WorkPlacement::WorkersFirst => state.workers.board.remote_can_run(&needs()),
    }
}

/// How a channel on a worker ended without a stack.
pub(super) enum NotStacked {
    /// No worker could take it, and this server may stack it instead.
    NoWorker(String),
    Failed(String),
}

/// What a channel a worker stacks needs from this server's preparation.
pub(super) struct RemoteChannel<'a> {
    pub job_id: &'a str,
    pub automatic: bool,
    pub group: &'a PreparedGroup,
    pub plan: &'a crate::calibration::CalibrationPlan,
    pub checkpoint: Option<resume::ResumeState>,
    pub accepted_only: bool,
    pub scoring: StackScoringSettings,
    pub method: StackMethod,
    pub gpu: StackGpu,
    pub order: snr::StackFrameOrder,
    pub calibration_fingerprint: &'a str,
    pub identity: integrate::CheckpointIdentity,
    pub orientation: &'a integrate::Orientation,
    /// Where this server keeps the channel's stack, clipped map and
    /// checkpoint.
    pub outputs: &'a integrate::ChannelOutputs,
    pub cancel: &'a Stop,
}

/// An input's name: `prefix`, a number, and the file's own extension, so a
/// worker reads an XISF frame as one.
fn input_name(prefix: &str, index: usize, path: &FsPath) -> String {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .filter(|extension| {
            extension
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
        .unwrap_or_else(|| "fits".into());
    format!("{prefix}-{index:05}.{extension}")
}

/// Stack a channel on a worker, applying its events to `events`, and put
/// what it made where this server keeps it.
pub(super) fn run_on_worker(
    state: &Arc<AppState>,
    channel: RemoteChannel<'_>,
    events: &mut dyn ChannelEvents,
) -> Result<integrate::ChannelOutcome, NotStacked> {
    let failed = NotStacked::Failed;
    let ticket = state.workers.board.ticket().map_err(failed)?;
    let mut inputs: Vec<(String, PathBuf)> = Vec::new();
    let mut group = channel.group.clone();
    for (index, frame) in group.frames.iter_mut().enumerate() {
        let name = input_name("frame", index, &frame.path);
        inputs.push((name.clone(), frame.path.clone()));
        frame.path = PathBuf::from(name);
    }
    // A master several sessions share goes once.
    let mut masters: BTreeMap<PathBuf, String> = BTreeMap::new();
    let sessions = channel
        .plan
        .sessions
        .iter()
        .map(|session| SessionSpec {
            recipe: session.recipe.with_files(|path| {
                let next = masters.len();
                let name = masters
                    .entry(path.to_path_buf())
                    .or_insert_with(|| {
                        let name = input_name("master", next, path);
                        inputs.push((name.clone(), path.to_path_buf()));
                        name
                    })
                    .clone();
                PathBuf::from(name)
            }),
            applied: session.applied.clone(),
            match_tolerances: session.match_tolerances,
        })
        .collect::<Vec<_>>();
    let checkpoint = channel.checkpoint.map(|checkpoint| {
        inputs.push((CHECKPOINT_CONTEXT.into(), checkpoint.context_path));
        checkpoint.manifest
    });
    let spec = ChannelSpec {
        job_id: channel.job_id.into(),
        group,
        assignments: channel.plan.assignments.clone(),
        sessions,
        applied: channel.plan.applied.clone(),
        checkpoint,
        accepted_only: channel.accepted_only,
        scoring: channel.scoring,
        method: channel.method,
        gpu: channel.gpu,
        order: channel.order,
        calibration_fingerprint: channel.calibration_fingerprint.into(),
        target_id: channel.identity.target_id,
        filter_name: channel.identity.filter_name.clone(),
        exposure_group_key: channel.identity.exposure_group_key.clone(),
        orientation: WireOrientation::from_orientation(channel.orientation),
    };
    let frames = channel.group.frames.len();
    ticket
        .submit(NewTask {
            kind: TaskKind::StackChannel,
            label: format!(
                "Stack of {} frames, {} channel",
                frames, channel.identity.filter_name
            ),
            person: !channel.automatic,
            needs: needs(),
            spec: serde_json::to_value(&spec).map_err(|error| failed(error.to_string()))?,
            inputs,
            outputs: [STACK, CLIPPED, CHECKPOINT_CONTEXT, CHECKPOINT_MANIFEST]
                .into_iter()
                .map(|name| (name.to_string(), false))
                .collect(),
        })
        .map_err(failed)?;
    // The progress and the events both reach the card.
    let events = std::cell::RefCell::new(events);
    let mut worker_named = None;
    let waited = ticket.wait_with(Waiting {
        stopped: &|| channel.cancel.requested(),
        progress: &mut |_, worker| {
            if let Some(worker) = worker
                && worker_named.as_deref() != Some(worker)
            {
                worker_named = Some(worker.to_string());
                events
                    .borrow_mut()
                    .resume_note(&format!("Stacked on {worker}"));
            }
        },
        events: &mut |event| match serde_json::from_value::<ChannelEvent>(event) {
            Ok(event) => event.apply(&mut **events.borrow_mut()),
            Err(error) => tracing::warn!("A stack event from a worker could not be read: {error}"),
        },
        finish_on_stop: Some(FINISH_ON_STOP),
    });
    let finished = match waited {
        Ok(finished) => finished,
        // However the worker ended after a stop, the build was stopped.
        Err(_) if channel.cancel.requested() => return Ok(integrate::ChannelOutcome::Cancelled),
        Err(WaitError::Stopped) => return Ok(integrate::ChannelOutcome::Cancelled),
        Err(WaitError::Failed(message)) if message.starts_with("No connected worker") => {
            return Err(NotStacked::NoWorker(message));
        }
        Err(WaitError::Failed(message)) => return Err(failed(message)),
    };
    let result: ChannelResult = serde_json::from_value(finished.result).map_err(|error| {
        failed(format!(
            "{} sent back a result that could not be read: {error}",
            finished.worker
        ))
    })?;
    let outputs = channel.outputs;
    // The checkpoint, when the worker wrote both halves, replaces this
    // server's; otherwise a later build starts over. A quality-ordered
    // build writes none and keeps the capture-order one, as on this server.
    if channel.order.resumable() {
        match (
            finished.outputs.get(CHECKPOINT_CONTEXT),
            finished.outputs.get(CHECKPOINT_MANIFEST),
        ) {
            (Some(context), Some(manifest)) => {
                place(context, &outputs.checkpoint_context).map_err(failed)?;
                place(manifest, &outputs.checkpoint_manifest).map_err(failed)?;
            }
            _ => {
                let _ = std::fs::remove_file(&outputs.checkpoint_context);
                let _ = std::fs::remove_file(&outputs.checkpoint_manifest);
            }
        }
    }
    if !result.built {
        return Ok(integrate::ChannelOutcome::Cancelled);
    }
    let stack = finished
        .outputs
        .get(STACK)
        .ok_or_else(|| failed(format!("{} sent back no stack", finished.worker)))?;
    // The map goes first, so a stack is never published beside an earlier
    // build's map.
    match finished.outputs.get(CLIPPED) {
        Some(clipped) => place(clipped, &outputs.clipped).map_err(failed)?,
        None => write_clipped(&outputs.clipped, None).map_err(failed)?,
    }
    place(stack, &outputs.fits).map_err(failed)?;
    let image = crate::image_io::open_linear_frame(&outputs.fits)
        .map_err(|error| failed(error.to_string()))?
        .image;
    Ok(integrate::ChannelOutcome::Built(Box::new(
        integrate::BuiltChannel {
            image,
            clipped: read_clipped(&outputs.clipped),
            progressive: result
                .progressive
                .ok_or_else(|| failed(format!("{} sent back no SNR curve", finished.worker)))?,
        },
    )))
}

/// Move a task's output to `destination`, through a temporary file beside
/// it: a rename when both are on one disk, a copy otherwise.
fn place(source: &FsPath, destination: &FsPath) -> Result<(), String> {
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let temporary = destination.with_extension(format!("{}.tmp", temporary_tag()));
    if std::fs::rename(source, &temporary).is_err() {
        std::fs::copy(source, &temporary).map_err(|error| {
            format!(
                "{} could not be copied to {}: {error}",
                source.display(),
                temporary.display()
            )
        })?;
    }
    std::fs::rename(&temporary, destination).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("{} could not be written: {error}", destination.display())
    })
}

/// Stack one synthetic channel on this server and on a worker, for the
/// worker command's tests, which run a real worker against a real server.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// What a stacked channel made, to compare.
    pub(crate) struct Stacked {
        pub pixels: Vec<f32>,
        pub dispositions: Vec<String>,
        pub matched_stars: Vec<Option<usize>>,
        pub notes: Vec<String>,
        pub checkpoint: bool,
    }

    #[derive(Default)]
    struct Recorder {
        decisions: Vec<StackFrameDecision>,
        notes: Vec<String>,
    }

    impl ChannelEvents for Recorder {
        fn calibration_note(&mut self, note: String) {
            self.notes.push(note);
        }
        fn resume_note(&mut self, note: &str) {
            self.notes.push(note.into());
        }
        fn checkpoint_unusable(&mut self, why: &str) {
            self.notes.push(why.into());
        }
        fn frames_restored(&mut self, restored: RestoredFrames) {
            self.decisions = restored.frames;
        }
        fn frame(&mut self, decision: StackFrameDecision, _: f64) {
            self.decisions.push(decision);
        }
        fn snr(&mut self, _: snr::ProgressiveSnr) {}
        fn phase(&mut self, _: String, _: Option<FinalPassProgress>) {}
        fn integrated_fractions(&mut self, _: Vec<f32>) {}
        fn sky_orientation(&mut self, _: StackSkyOrientation) {}
        fn live(&mut self, _: &seiza_stacking::LiveStacker, _: u32, _: bool) {}
        fn batch_frames(&self) -> usize {
            usize::MAX
        }
        fn observe_batch(&mut self, _: std::time::Duration, _: usize) {}
    }

    /// A channel's frames, for the tests outside this module.
    #[derive(Clone)]
    pub(crate) struct Channel(PreparedGroup);

    /// Five 384×384 star fields of one patch of sky, each shifted a few
    /// pixels, with noise of their own.
    pub(crate) fn synthetic_group(directory: &FsPath) -> Channel {
        let (width, height) = (384usize, 384usize);
        let mut seed = 11u64;
        let mut random = move || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 33) as f64 / (1u64 << 31) as f64
        };
        let stars = (0..90)
            .map(|_| {
                (
                    20.0 + random() * (width as f64 - 40.0),
                    20.0 + random() * (height as f64 - 40.0),
                    2000.0 + random() * 30000.0,
                )
            })
            .collect::<Vec<_>>();
        let shifts = [
            (0.0, 0.0),
            (3.0, 2.0),
            (-2.0, 4.0),
            (5.0, -3.0),
            (-4.0, -1.0),
        ];
        let mut frames = Vec::new();
        for (index, (dx, dy)) in shifts.into_iter().enumerate() {
            let mut data = vec![0.0f32; width * height];
            for (pixel, sample) in data.iter_mut().enumerate() {
                let (x, y) = ((pixel % width) as f64, (pixel / width) as f64);
                let mut value = 1000.0 + (random() - 0.5) * 30.0;
                for (sx, sy, flux) in &stars {
                    let (ex, ey) = (x - (sx + dx), y - (sy + dy));
                    let distance = ex * ex + ey * ey;
                    if distance < 64.0 {
                        value += flux * (-distance / (2.0 * 1.6 * 1.6)).exp();
                    }
                }
                *sample = value as f32;
            }
            let image = seiza_stacking::LinearImage::new(width, height, 1, data).unwrap();
            let path = directory.join(format!("light-{index}.fits"));
            seiza_stacking::write_processed_image_fits_f32(&path, &image, &[], &[]).unwrap();
            frames.push(PreparedFrame {
                image_id: index as i32,
                acquired_date: None,
                quality_score: None,
                source_fingerprint: source_fingerprint(&path),
                path,
                expected_target: None,
                exposure_seconds: 120.0,
                read_fingerprint: None,
            });
        }
        Channel(PreparedGroup {
            index: 0,
            calibration: crate::calibration::CalibrationMode::Off,
            flat_star_masking: false,
            frames,
        })
    }

    fn outputs(directory: &FsPath) -> integrate::ChannelOutputs {
        integrate::ChannelOutputs {
            fits: directory.join("stack.fits"),
            clipped: directory.join("clipped.fits"),
            checkpoint_context: directory.join("resume.ctx"),
            checkpoint_manifest: directory.join("resume.json"),
            scratch: directory.join("scratch"),
        }
    }

    fn identity() -> integrate::CheckpointIdentity {
        integrate::CheckpointIdentity {
            target_id: 1,
            filter_name: "L".into(),
            exposure_group_key: None,
        }
    }

    fn stacked(recorder: Recorder, outputs: &integrate::ChannelOutputs) -> Stacked {
        Stacked {
            pixels: crate::image_io::open_linear_frame(&outputs.fits)
                .unwrap()
                .image
                .data,
            dispositions: recorder
                .decisions
                .iter()
                .map(|decision| decision.disposition.clone())
                .collect(),
            matched_stars: recorder
                .decisions
                .iter()
                .map(|decision| decision.matched_stars)
                .collect(),
            notes: recorder.notes,
            checkpoint: outputs.checkpoint_context.exists() && outputs.checkpoint_manifest.exists(),
        }
    }

    /// Stack `group` on this server, as a build does.
    pub(crate) fn stack_here(Channel(group): &Channel, directory: &FsPath) -> Stacked {
        let plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        let outputs = outputs(directory);
        let pool = crate::concurrency::ComputePool::take("stack-test", 4).unwrap();
        let mut recorder = Recorder::default();
        let built = integrate::integrate_channel(
            integrate::ChannelWork {
                job_id: "test",
                group,
                plan: &plan,
                checkpoint: None,
                accepted_only: true,
                scoring: StackScoringSettings::default(),
                method: StackMethod::classic(),
                gpu: StackGpu::default(),
                order: snr::StackFrameOrder::Capture,
                calibration_fingerprint: &plan.applied.fingerprint,
                identity: identity(),
                orientation: integrate::Orientation::Reference { anchor: None },
                outputs: outputs.clone(),
                pool: &pool,
                threads: execution::ThreadBudget::from_total(4),
                available_memory: crate::concurrency::available_memory_bytes(),
                worker_policy: &crate::concurrency::WorkerPolicy::default(),
                cancel: &Stop::new(),
            },
            &mut recorder,
        )
        .unwrap();
        assert!(matches!(built, integrate::ChannelOutcome::Built(_)));
        stacked(recorder, &outputs)
    }

    /// Stack `group` on a worker connected to `state`'s board.
    pub(crate) fn stack_on_worker(
        state: &Arc<AppState>,
        Channel(group): &Channel,
        directory: &FsPath,
    ) -> Stacked {
        let plan = crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
        let outputs = outputs(directory);
        let mut recorder = Recorder::default();
        let built = run_on_worker(
            state,
            RemoteChannel {
                job_id: "test",
                automatic: false,
                group,
                plan: &plan,
                checkpoint: None,
                accepted_only: true,
                scoring: StackScoringSettings::default(),
                method: StackMethod::classic(),
                gpu: StackGpu::default(),
                order: snr::StackFrameOrder::Capture,
                calibration_fingerprint: &plan.applied.fingerprint,
                identity: identity(),
                orientation: &integrate::Orientation::Reference { anchor: None },
                outputs: &outputs,
                cancel: &Stop::new(),
            },
            &mut recorder,
        );
        match built {
            Ok(integrate::ChannelOutcome::Built(_)) => {}
            Ok(integrate::ChannelOutcome::Cancelled) => panic!("the worker stopped"),
            Err(NotStacked::NoWorker(message) | NotStacked::Failed(message)) => panic!("{message}"),
        }
        stacked(recorder, &outputs)
    }
}
