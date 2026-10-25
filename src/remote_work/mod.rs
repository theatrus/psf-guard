//! Work that runs on a worker: a `psf-guard worker` process on another
//! machine, or this server's own worker. The server's task board hands each
//! worker tasks it can run; see `docs/design/remote-workers.md`.
//!
//! This module holds what both sides share: the wire protocol, and how each
//! kind of task runs on whichever worker claims it.

pub mod protocol;
pub mod rc_astro;

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use protocol::TaskKind;

/// The tools installed on a worker.
#[derive(Clone, Default)]
pub struct LocalTools {
    pub rc_astro: Option<seiza_stacking::RcAstroCli>,
    /// How many tasks the worker runs at once; a stack channel takes this
    /// share of its cores and memory.
    pub slots: usize,
}

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
}

impl Default for WorkChoice {
    fn default() -> Self {
        Self {
            rc_astro_tools: None,
            stack: true,
        }
    }
}

/// The `--work` choices, for the command's help and its errors.
pub const WORK_CHOICES: &str = "stack, rc-astro (every RC-Astro tool), bxt, nxt or sxt";

impl WorkChoice {
    /// Read `--work` values: `rc-astro` takes every RC-Astro tool, and a
    /// tool's key, such as `bxt`, takes that tool.
    pub fn parse(values: &[String]) -> Result<Self, String> {
        if values.is_empty() {
            return Ok(Self::default());
        }
        let mut tools = Vec::new();
        let mut stack = false;
        for value in values {
            let value = value.trim().to_ascii_lowercase();
            match value.as_str() {
                "stack" => stack = true,
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
        })
    }

    /// RC-Astro tools only, as this server's own worker takes: this server
    /// stacks its builds in process.
    pub fn rc_astro_only() -> Self {
        Self {
            rc_astro_tools: None,
            stack: false,
        }
    }

    pub fn takes_stack(&self) -> bool {
        self.stack
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
