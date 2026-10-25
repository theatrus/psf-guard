//! What a worker and the server send each other over HTTP, under
//! `/api/workers/v1/`. Both sides use these types, so the `psf-guard worker`
//! command and the server it joins always agree on the shape.
//!
//! A worker authenticates every request with `Authorization: Bearer psfw_…`.
//! Requests about one task also send the lease the worker holds on it in
//! [`LEASE_HEADER`]; the server refuses a request whose lease is no longer
//! current.

use serde::{Deserialize, Serialize};

/// The prefix of a worker token. A worker token can register a worker and
/// run the tasks it claims, and nothing else.
pub const TOKEN_PREFIX: &str = "psfw_";

/// Holds the lease id on every request about a task.
pub const LEASE_HEADER: &str = "x-psf-worker-lease";

/// Holds the SHA-256 digest, in lowercase hex, of an uploaded output.
pub const DIGEST_HEADER: &str = "x-psf-sha256";

/// The protocol version. A server refuses a worker that speaks another one,
/// and the error message explains which release to install.
pub const PROTOCOL_VERSION: u32 = 1;

/// `POST /api/workers/v1/register`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registration {
    pub protocol: u32,
    /// The worker's name, as Settings › Workers lists it.
    pub name: String,
    /// The PSF Guard release the worker runs.
    pub version: String,
    pub offers: Offers,
    pub files: FileAccess,
}

/// What a worker can run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Offers {
    /// Processor threads the worker may use.
    pub threads: usize,
    /// The kinds of task the worker takes: what it can run, or the part of
    /// that its `--work` choices list.
    #[serde(default)]
    pub work: Vec<TaskKind>,
    /// The RC-Astro tools the worker takes, with their live schemas. Empty
    /// when rc-astro is not installed there or `--work` leaves it out.
    #[serde(default)]
    pub rc_astro: Vec<seiza_stacking::ExternalToolSchema>,
}

/// How a worker reaches the files a task names.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileAccess {
    pub mode: FileMode,
    /// Where the server's folders are on the worker, for shared mode. Empty
    /// in shared mode means the worker sees them at the same paths.
    #[serde(default)]
    pub maps: Vec<PathMap>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileMode {
    /// Every input is downloaded from the server and every output uploaded
    /// to it.
    #[default]
    Remote,
    /// The worker reads inputs and writes outputs on storage it shares with
    /// the server. A file it cannot see there goes through the server.
    Shared,
}

/// One server folder and where the worker sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMap {
    pub server: String,
    pub local: String,
}

/// The server's answer to a registration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Registered {
    /// The session id every later request names.
    pub session: String,
    /// Seconds a lease lasts without a progress report.
    pub lease_seconds: u64,
    /// Seconds the server holds a claim open before it answers that no
    /// work is ready.
    pub poll_seconds: u64,
    /// For a worker in shared mode: a file the server wrote in its exchange
    /// folder, for the worker to read and answer. See [`ProbeAnswer`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub probe: Option<ExchangeProbe>,
}

/// A file in the server's exchange folder, which tasks' inputs and outputs
/// go through. A worker that can read it and write the reply beside it gets
/// those files by path; one that cannot gets them over HTTP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExchangeProbe {
    /// The probe file's path on the server.
    pub path: String,
    /// The path the reply goes to on the server.
    pub reply_path: String,
}

/// `POST /api/workers/v1/sessions/{session}/probe`: what the worker read
/// from the probe file, after it wrote the same bytes to the reply path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeAnswer {
    /// The probe file's SHA-256 digest, or `None` when the worker could not
    /// read it.
    pub digest: Option<String>,
    /// Why the worker could not read the probe or write the reply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// Whether the worker shares the exchange folder with the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub shared: bool,
    /// Why the server sends this worker's files over HTTP instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `POST /api/workers/v1/sessions/{session}/claim`: a task for the worker,
/// answered with `200`. `204` means no task was ready within the poll time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub task: String,
    pub lease: String,
    pub kind: TaskKind,
    /// What the task is, for the worker's log.
    pub label: String,
    /// The task's spec, as its kind defines it.
    pub spec: serde_json::Value,
    pub inputs: Vec<InputFile>,
    pub outputs: Vec<OutputFile>,
}

/// The kinds of task a worker can run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    /// One RC-Astro tool over one image. See
    /// [`crate::remote_work::rc_astro`].
    RcAstroStep,
}

/// A file the task reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputFile {
    pub name: String,
    pub size: u64,
    /// The file's path on the server, sent to a worker in shared mode. The
    /// worker reads it there when its maps reach it and the size matches,
    /// and downloads it otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// A file the task writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputFile {
    pub name: String,
    /// Whether the task fails without it.
    pub required: bool,
    /// Where the worker writes it on the server, sent when the worker shares
    /// the exchange folder. Otherwise the worker uploads it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `POST …/tasks/{task}/progress`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    /// How much of the task is done, from 0 to 1.
    pub fraction: f32,
}

/// The server's answer to a progress report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressReply {
    /// The server no longer wants the task: the worker stops it and sends
    /// nothing more about it.
    pub stop: bool,
}

/// `POST …/tasks/{task}/complete`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Completion {
    /// The task's result, as its kind defines it.
    pub result: serde_json::Value,
    /// Every output the worker wrote or uploaded.
    pub outputs: Vec<WrittenOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WrittenOutput {
    pub name: String,
    pub size: u64,
    /// SHA-256 in lowercase hex. The server checks it before it uses the
    /// file.
    pub sha256: String,
}

/// `POST …/tasks/{task}/fail`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Failure {
    pub error: String,
}

/// A FITS header card, for sending headers inside a spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HeaderCard {
    pub key: String,
    pub value: HeaderValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum HeaderValue {
    Logical(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Raw(String),
}

impl HeaderCard {
    pub fn from_fits(cards: &[(String, seiza_fits::HeaderValue)]) -> Vec<Self> {
        cards
            .iter()
            .map(|(key, value)| Self {
                key: key.clone(),
                value: match value {
                    seiza_fits::HeaderValue::Logical(value) => HeaderValue::Logical(*value),
                    seiza_fits::HeaderValue::Integer(value) => HeaderValue::Integer(*value),
                    seiza_fits::HeaderValue::Float(value) => HeaderValue::Float(*value),
                    seiza_fits::HeaderValue::String(value) => HeaderValue::String(value.clone()),
                    seiza_fits::HeaderValue::Raw(value) => HeaderValue::Raw(value.clone()),
                },
            })
            .collect()
    }

    pub fn to_fits(cards: &[Self]) -> Vec<(String, seiza_fits::HeaderValue)> {
        cards
            .iter()
            .map(|card| {
                (
                    card.key.clone(),
                    match &card.value {
                        HeaderValue::Logical(value) => seiza_fits::HeaderValue::Logical(*value),
                        HeaderValue::Integer(value) => seiza_fits::HeaderValue::Integer(*value),
                        HeaderValue::Float(value) => seiza_fits::HeaderValue::Float(*value),
                        HeaderValue::String(value) => {
                            seiza_fits::HeaderValue::String(value.clone())
                        }
                        HeaderValue::Raw(value) => seiza_fits::HeaderValue::Raw(value.clone()),
                    },
                )
            })
            .collect()
    }
}

/// Whether `name` may name a task's input or output file: lowercase letters,
/// digits, dots, dashes and underscores, not starting with a dot. A name
/// never reaches outside the task's folder.
pub fn valid_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_cards_survive_the_trip_through_json() {
        let cards = vec![
            (
                "ROWORDER".to_string(),
                seiza_fits::HeaderValue::String("TOP-DOWN".into()),
            ),
            ("EXPTIME".to_string(), seiza_fits::HeaderValue::Float(300.0)),
            ("STACKCNT".to_string(), seiza_fits::HeaderValue::Integer(42)),
            (
                "FLIPPED".to_string(),
                seiza_fits::HeaderValue::Logical(true),
            ),
            ("ODD".to_string(), seiza_fits::HeaderValue::Raw("".into())),
        ];
        let json = serde_json::to_string(&HeaderCard::from_fits(&cards)).unwrap();
        let back: Vec<HeaderCard> = serde_json::from_str(&json).unwrap();
        assert_eq!(HeaderCard::to_fits(&back), cards);
    }

    #[test]
    fn file_names_stay_inside_the_task_folder() {
        for name in ["processed.fits", "stars.fits", "input-1.fits", "a_b"] {
            assert!(valid_file_name(name), "{name}");
        }
        for name in [
            "",
            "../x",
            "a/b",
            ".hidden",
            "Upper.fits",
            "a\\b",
            &"x".repeat(65),
        ] {
            assert!(!valid_file_name(name), "{name}");
        }
    }
}
