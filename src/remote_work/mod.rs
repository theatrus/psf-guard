//! Work that runs on a worker: a `psf-guard worker` process on another
//! machine, or this server's own worker. The server's task board hands each
//! worker tasks it can run; see `docs/design/remote-workers.md`.
//!
//! This module holds what both sides share: the wire protocol, and how each
//! kind of task runs on whichever worker claims it.

pub mod noise_model;
pub mod protocol;
pub mod rc_astro;
pub mod steps;

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
        // Every engine reports the providers it can run here.
        let mut steps = Vec::new();
        let mut problem = None;
        for engine in [steps::Engine::RcAstro, steps::Engine::SeizaNoiseModel] {
            let (schemas, engine_problem) = steps::runner(engine).offers(self);
            problem = problem.or(engine_problem);
            steps.extend(
                schemas
                    .into_iter()
                    .filter(|schema| choice.takes_provider(&schema.key))
                    .filter_map(steps::StepOffer::of),
            );
        }
        // Servers older than providers read the RC-Astro tools here, and
        // `work` as the kinds they know.
        let rc_astro: Vec<_> = steps
            .iter()
            .filter(|offer| {
                steps::provider(&offer.provider)
                    .is_some_and(|found| found.engine == steps::Engine::RcAstro)
            })
            .map(|offer| offer.schema.clone())
            .collect();
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
                steps,
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

    /// Whether it takes a provider's steps, by the provider's key.
    pub fn takes_provider(&self, provider: &str) -> bool {
        match steps::provider(provider).map(|found| found.engine) {
            Some(steps::Engine::RcAstro) => self.takes_rc_astro_tool(provider),
            Some(steps::Engine::SeizaNoiseModel) => self.takes_noise_model(),
            None => false,
        }
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
        // What a server older than providers sends: an RC-Astro step, whose
        // spec is the RC-Astro engine's payload.
        TaskKind::RcAstroStep => steps::runner(steps::Engine::RcAstro).run(
            spec,
            steps::RunContext {
                inputs,
                output_dir,
                tools,
                stop,
                report,
            },
        ),
        TaskKind::ProcessingStep => {
            let spec: steps::StepSpec = serde_json::from_value(spec)
                .map_err(|error| format!("The processing step could not be read: {error}"))?;
            let provider = steps::provider(&spec.provider).ok_or_else(|| {
                format!(
                    "This worker does not know the provider {:?}; update PSF Guard on it",
                    spec.provider
                )
            })?;
            steps::runner(provider.engine).run(
                spec.payload,
                steps::RunContext {
                    inputs,
                    output_dir,
                    tools,
                    stop,
                    report,
                },
            )
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
        let model = |offers: &protocol::Offers| {
            offers
                .steps
                .iter()
                .find(|offer| offer.provider == noise_model::TOOL)
                .map(|offer| (offer.category, offer.schema.cli_version.clone()))
        };
        let installed = noise_model::testing::FixedEngine::new("noise-v1");
        let offered = offers(installed.clone(), &[]);
        assert_eq!(
            model(&offered),
            Some((
                steps::Category::Denoise,
                noise_model::testing::identity("noise-v1").version()
            ))
        );
        // A server older than providers reads `work` as a list of the kinds
        // it knows, and a registration with a kind it does not know fails.
        assert!(!offered.work.contains(&TaskKind::ProcessingStep));
        let wire = serde_json::to_value(&offered).unwrap();
        assert_eq!(wire["work"], serde_json::json!(["stack_channel"]));
        assert_eq!(model(&offers(installed, &["stack".into()])), None);
        let missing = noise_model::testing::FixedEngine::missing("ONNX Runtime is not installed");
        assert_eq!(model(&offers(missing, &[])), None);
    }

    #[test]
    fn a_worker_older_than_providers_offers_its_rc_astro_tools_as_providers() {
        // What a worker of an earlier release registers with.
        let old: protocol::Offers = serde_json::from_value(serde_json::json!({
            "threads": 4,
            "work": ["rc_astro_step"],
            "rc_astro": [{
                "schema_version": 6, "cli_version": "2.6.6", "key": "nxt",
                "name": "RC-Astro NoiseXTerminator", "ml_version": 3, "licensed": true,
                "license_message": null, "parameters": [],
            }],
        }))
        .unwrap();
        assert!(old.steps.is_empty());
        let read = steps::offers_of(&old);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].provider, "nxt");
        assert_eq!(read[0].category, steps::Category::Denoise);
        // One that left RC-Astro out of `--work` offers none.
        let narrowed = protocol::Offers {
            work: vec![TaskKind::StackChannel],
            ..old
        };
        assert!(steps::offers_of(&narrowed).is_empty());
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
