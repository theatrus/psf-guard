//! Work that runs on a worker: a `psf-guard worker` process on another
//! machine, or this server's own worker. The server's task board hands each
//! worker tasks it can run; see `docs/design/remote-workers.md`.
//!
//! This module holds what both sides share: the wire protocol, and how each
//! kind of task runs on whichever worker claims it.

pub mod noise_model;
pub mod protocol;
pub mod rc_astro;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use protocol::TaskKind;

/// The tools installed on a worker.
#[derive(Clone, Default)]
pub struct LocalTools {
    pub rc_astro: Option<seiza_stacking::RcAstroCli>,
    /// What runs the noise model here. `None` on a worker that leaves it
    /// out.
    pub noise_model: Option<Arc<dyn noise_model::Engine>>,
    /// The threads a noise model run may use, given whether a person waits
    /// for it. This server leases them from its shared budget; `None` uses
    /// this machine's share of its cores.
    pub compute: Option<ComputeSource>,
    /// How many tasks the worker runs at once; a stack channel takes this
    /// share of its cores and memory.
    pub slots: usize,
}

/// Threads granted to one task. Dropping `hold` gives them back.
pub struct Compute {
    pub threads: usize,
    pub hold: Option<Box<dyn std::any::Any + Send>>,
}

/// Grants threads to a task, given whether a person waits for it.
pub type ComputeSource = Arc<dyn Fn(bool) -> Compute + Send + Sync>;

impl LocalTools {
    /// What the worker takes, for its registration: what it can run,
    /// narrowed to `choice`. Spawns short-lived processes to read the
    /// RC-Astro schemas.
    pub fn offers(
        &self,
        threads: usize,
        choice: &WorkChoice,
    ) -> (protocol::Offers, Option<String>) {
        let (mut rc_astro, problem) = match &self.rc_astro {
            Some(cli) => rc_astro::probe(cli),
            None => (Vec::new(), None),
        };
        // What is missing is not a problem to report: the model is an
        // optional download.
        let noise_model = self
            .noise_model
            .as_ref()
            .filter(|_| choice.takes_noise_model())
            .and_then(|engine| engine.identity().ok());
        rc_astro.retain(|schema| choice.takes_rc_astro_tool(&schema.key));
        let mut work = Vec::new();
        if !rc_astro.is_empty() {
            work.push(TaskKind::RcAstroStep);
        }
        if choice.takes_stack() {
            work.push(TaskKind::StackChannel);
        }
        (
            protocol::Offers {
                threads,
                slots: 1,
                work,
                rc_astro,
                stacking_version: choice
                    .takes_stack()
                    .then(|| crate::server::stack_preview::SEIZA_STACKING_OUTPUT_VERSION.into()),
                stacking_release: choice
                    .takes_stack()
                    .then(|| env!("CARGO_PKG_VERSION").into()),
                noise_model,
            },
            problem,
        )
    }
}

/// What a running task tells whoever runs it.
pub trait TaskReport: Send {
    /// How much of the task is done, from 0 to 1.
    fn fraction(&mut self, done: f32);
    /// Something that happened, as the task's kind defines it.
    fn event(&mut self, event: serde_json::Value);
    /// A running image of the work, as a PNG, integrating `frames` frames.
    /// Only the newest is sent, and one that cannot be sent is dropped.
    fn preview(&mut self, _png: Vec<u8>, _frames: u32) {}
}

/// The work a worker takes, from its `--work` choices: everything it can
/// run when there are none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkChoice {
    /// The RC-Astro tools it takes, or `None` for every one.
    rc_astro_tools: Option<Vec<String>>,
    /// Whether it takes stack builds.
    stack: bool,
    /// Whether it runs the noise model.
    noise_model: bool,
}

impl Default for WorkChoice {
    fn default() -> Self {
        Self {
            rc_astro_tools: None,
            stack: true,
            noise_model: true,
        }
    }
}

/// The `--work` choices, for the command's help and its errors.
pub const WORK_CHOICES: &str =
    "stack, noise-model, rc-astro (every RC-Astro tool), bxt, nxt or sxt";

impl WorkChoice {
    /// Read `--work` values: `rc-astro` takes every RC-Astro tool, and a
    /// tool's key, such as `bxt`, takes that tool.
    pub fn parse(values: &[String]) -> Result<Self, String> {
        if values.is_empty() {
            return Ok(Self::default());
        }
        let mut tools = Vec::new();
        let mut stack = false;
        let mut noise_model = false;
        for value in values {
            let value = value.trim().to_ascii_lowercase();
            match value.as_str() {
                "stack" => stack = true,
                noise_model::TOOL => noise_model = true,
                "rc-astro" => tools.extend(rc_astro::TOOLS.iter().map(|tool| tool.to_string())),
                tool if rc_astro::TOOLS.contains(&tool) => tools.push(tool.to_string()),
                _ => return Err(format!("--work must be {WORK_CHOICES}, not '{value}'")),
            }
        }
        tools.sort();
        tools.dedup();
        Ok(Self {
            rc_astro_tools: Some(tools),
            stack,
            noise_model,
        })
    }

    /// The processing steps only, as this server's own worker takes: the
    /// RC-Astro tools and the noise model. This server stacks its builds
    /// in process.
    pub fn processing_only() -> Self {
        Self {
            rc_astro_tools: None,
            stack: false,
            noise_model: true,
        }
    }

    pub fn takes_stack(&self) -> bool {
        self.stack
    }

    pub fn takes_noise_model(&self) -> bool {
        self.noise_model
    }

    /// Whether it takes any RC-Astro tool.
    pub fn takes_rc_astro(&self) -> bool {
        self.rc_astro_tools
            .as_ref()
            .is_none_or(|tools| !tools.is_empty())
    }

    pub fn takes_rc_astro_tool(&self, tool: &str) -> bool {
        self.rc_astro_tools
            .as_ref()
            .is_none_or(|tools| tools.iter().any(|taken| taken == tool))
    }
}

/// Run a task on this machine. `inputs` gives where each input is;
/// outputs are written into `output_dir` under their names, and scratch
/// files go in `work_dir`. Returns the task's result.
#[allow(clippy::too_many_arguments)]
pub fn execute(
    kind: TaskKind,
    spec: serde_json::Value,
    inputs: &BTreeMap<String, PathBuf>,
    output_dir: &Path,
    work_dir: &Path,
    tools: &LocalTools,
    stop: &crate::concurrency::Stop,
    report: &mut dyn TaskReport,
) -> Result<serde_json::Value, String> {
    match kind {
        TaskKind::RcAstroStep => {
            let spec: rc_astro::StepSpec = serde_json::from_value(spec)
                .map_err(|error| format!("The RC-Astro task could not be read: {error}"))?;
            let cli = tools
                .rc_astro
                .as_ref()
                .ok_or_else(|| "rc-astro is not installed on this worker".to_string())?;
            let input = inputs
                .get(rc_astro::INPUT)
                .ok_or_else(|| "The RC-Astro task has no input image".to_string())?;
            let cancel = {
                let stop = stop.clone();
                seiza_stacking::CancelSignal::new(move || stop.requested())
            };
            let result = rc_astro::run(cli, &spec, input, output_dir, &cancel, &mut |done| {
                report.fraction(done)
            })?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        }
        TaskKind::NoiseModelStep => {
            let spec: noise_model::StepSpec = serde_json::from_value(spec)
                .map_err(|error| format!("The noise model task could not be read: {error}"))?;
            let engine = tools
                .noise_model
                .as_ref()
                .ok_or_else(|| "The noise model is not installed on this worker".to_string())?;
            let input = inputs
                .get(noise_model::INPUT)
                .ok_or_else(|| "The noise model task has no input image".to_string())?;
            let compute = match &tools.compute {
                Some(source) => source(spec.person),
                None => Compute {
                    threads: (crate::concurrency::logical_cores() / tools.slots.max(1)).max(1),
                    hold: None,
                },
            };
            // Tiles finish on Seiza's threads, so the report is shared with
            // them. Each whole percent is reported once.
            let report = std::sync::Mutex::new((report, 0.0f32));
            let stopped = || stop.requested();
            let result = noise_model::run(
                engine.as_ref(),
                &spec,
                input,
                output_dir,
                compute.threads,
                &stopped,
                &|fraction| {
                    let mut report = report
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if fraction >= report.1 + 0.01 || fraction >= 1.0 {
                        report.1 = fraction;
                        report.0.fraction(fraction);
                    }
                },
            )?;
            drop(compute.hold);
            serde_json::to_value(result).map_err(|error| error.to_string())
        }
        TaskKind::StackChannel => crate::server::stack_preview::remote_channel::execute(
            spec,
            inputs,
            output_dir,
            work_dir,
            tools.slots,
            stop,
            report,
        ),
    }
}

/// A file's SHA-256 in lowercase hex, and its size.
pub fn file_digest(path: &Path) -> std::io::Result<(String, u64)> {
    use sha2::{Digest, Sha256};
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    let mut size = 0u64;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((hex(&hasher.finalize()), size))
}

/// Bytes in lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

/// Where a server path is on this machine, through the first map whose
/// server folder holds it. `None` when no map reaches it. No maps means
/// the same paths.
pub fn map_path(maps: &[protocol::PathMap], server_path: &str) -> Option<PathBuf> {
    if maps.is_empty() {
        return Some(PathBuf::from(server_path));
    }
    let path = Path::new(server_path);
    maps.iter().find_map(|map| {
        let rest = path.strip_prefix(&map.server).ok()?;
        Some(Path::new(&map.local).join(rest))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::PathMap;

    #[test]
    fn a_map_moves_a_server_path_onto_the_worker() {
        let maps = vec![
            PathMap {
                server: "/srv/psf".into(),
                local: "/mnt/psf".into(),
            },
            PathMap {
                server: "/data".into(),
                local: "D:/data".into(),
            },
        ];
        assert_eq!(
            map_path(&maps, "/srv/psf/workers/t1/input.fits"),
            Some(PathBuf::from("/mnt/psf/workers/t1/input.fits"))
        );
        assert_eq!(
            map_path(&maps, "/data/x.fits"),
            Some(Path::new("D:/data").join("x.fits"))
        );
        // A path that only starts with the same letters is not inside.
        assert_eq!(map_path(&maps, "/srv/psfother/x"), None);
        assert_eq!(map_path(&[], "/a/b"), Some(PathBuf::from("/a/b")));
    }

    #[test]
    fn work_choices_narrow_the_rc_astro_tools() {
        let everything = WorkChoice::parse(&[]).unwrap();
        assert!(rc_astro::TOOLS
            .iter()
            .all(|tool| everything.takes_rc_astro_tool(tool)));
        let all = WorkChoice::parse(&["RC-Astro".into()]).unwrap();
        assert_eq!(all, everything_listed());
        let some = WorkChoice::parse(&["bxt".into(), "sxt".into(), "bxt".into()]).unwrap();
        assert!(some.takes_rc_astro_tool("bxt"));
        assert!(some.takes_rc_astro_tool("sxt"));
        assert!(!some.takes_rc_astro_tool("nxt"));
        let refused = WorkChoice::parse(&["gpu".into()]).unwrap_err();
        assert!(refused.contains("rc-astro"), "{refused}");
        // Listing RC-Astro tools leaves stacking out, and `stack` takes it.
        assert!(everything.takes_stack());
        assert!(!some.takes_stack());
        let stack = WorkChoice::parse(&["stack".into()]).unwrap();
        assert!(stack.takes_stack());
        assert!(!stack.takes_rc_astro_tool("bxt"));
        // The noise model is a choice of its own, beside the RC-Astro tools.
        assert!(everything.takes_noise_model());
        assert!(!all.takes_noise_model());
        assert!(!stack.takes_noise_model());
        let model = WorkChoice::parse(&["noise-model".into(), "nxt".into()]).unwrap();
        assert!(model.takes_noise_model());
        assert!(model.takes_rc_astro_tool("nxt"));
        assert!(!model.takes_stack());
    }

    #[test]
    fn a_worker_offers_the_noise_model_only_when_it_can_run_it() {
        let offers = |engine: Arc<dyn noise_model::Engine>, work: &[String]| {
            LocalTools {
                noise_model: Some(engine),
                slots: 1,
                ..Default::default()
            }
            .offers(4, &WorkChoice::parse(work).unwrap())
            .0
        };
        let installed = noise_model::testing::FixedEngine::new("noise-v1");
        let offered = offers(installed.clone(), &[]);
        assert_eq!(
            offered.noise_model,
            Some(noise_model::testing::identity("noise-v1"))
        );
        // An older server reads `work` as a list of the kinds it knows.
        assert!(!offered.work.contains(&TaskKind::NoiseModelStep));
        assert_eq!(offers(installed, &["stack".into()]).noise_model, None);
        let missing = noise_model::testing::FixedEngine::missing("ONNX Runtime is not installed");
        assert_eq!(offers(missing, &[]).noise_model, None);
        // A registration from before the model reads as offering none.
        let old: protocol::Offers = serde_json::from_str(r#"{"threads": 4}"#).unwrap();
        assert_eq!(old.noise_model, None);
    }

    fn everything_listed() -> WorkChoice {
        WorkChoice::parse(&["bxt".into(), "nxt".into(), "sxt".into()]).unwrap()
    }

    #[test]
    fn digests_are_lowercase_hex_of_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, b"abc").unwrap();
        let (digest, size) = file_digest(&path).unwrap();
        assert_eq!(size, 3);
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
