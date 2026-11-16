//! Workers: machines that run tasks for this server. See
//! `docs/design/remote-workers.md`.
//!
//! A worker joins with a token minted in Settings › Workers, registers what
//! it can run, and claims tasks from the [`board`]. This server runs its own
//! worker in process (`local`), which claims tasks through the same board,
//! so every task this server runs takes the path a remote one does.

pub mod board;
pub mod license;
mod local;
pub mod rc_astro;
pub mod routes;
pub mod tokens;

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::db_registry::WorkerTokenRecord;
use crate::remote_work::protocol::Offers;
use crate::remote_work::LocalTools;

pub use board::{Board, Finished, NewTask, TaskNeeds, TaskPreview, Ticket, WaitError, Waiting};

/// The token id this server's own worker registers under.
pub const THIS_SERVER: &str = "this-server";

/// How long what this server's worker offers is trusted before rc-astro is
/// probed again, so installing it needs no restart.
const LOCAL_OFFERS_TTL: Duration = Duration::from_secs(60);

/// The workers of one server: the tokens that may join, the board, and
/// this server's own worker.
pub struct Workers {
    pub board: Arc<Board>,
    tokens: RwLock<Vec<WorkerTokenRecord>>,
    local: OnceLock<local::LocalWorker>,
    /// When this server's offers were last probed, what they were, and the
    /// last problem reading a schema.
    local_offers: Mutex<Option<(Instant, Offers, Option<String>)>>,
    /// Finds the tools installed on this server.
    tools: RwLock<ToolSource>,
    /// The RC-Astro license checks people started.
    pub license_checks: Arc<license::LicenseChecks>,
}

/// Finds the tools installed on this server: [`local_tools`], or a test's
/// stand-ins.
pub(crate) type ToolSource = Arc<dyn Fn() -> LocalTools + Send + Sync>;

impl Default for Workers {
    fn default() -> Self {
        Self::with_board(Board::default())
    }
}

impl Workers {
    /// Workers on `board`, such as one with short leases.
    pub(crate) fn with_board(board: Board) -> Self {
        Self {
            board: Arc::new(board),
            tokens: RwLock::new(Vec::new()),
            local: OnceLock::new(),
            local_offers: Mutex::new(None),
            tools: RwLock::new(Arc::new(local_tools)),
            license_checks: Arc::default(),
        }
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        // This server's worker thread ends with its server.
        self.board.close();
    }
}

impl Workers {
    /// Take the tokens, the exchange folder and where each kind of work
    /// runs from the registry.
    pub fn configure(
        &self,
        settings: crate::db_registry::RemoteWorkerSettings,
        default_exchange: PathBuf,
    ) {
        self.board.set_exchange(
            settings
                .exchange_folder
                .map(PathBuf::from)
                .unwrap_or(default_exchange),
        );
        // Files of tasks a stopped server left behind are of no use.
        self.board.remove_stale_files();
        self.board.set_placement(
            crate::remote_work::protocol::TaskKind::RcAstroStep,
            settings.rc_astro,
        );
        self.board.set_placement(
            crate::remote_work::protocol::TaskKind::StackChannel,
            settings.stack,
        );
        self.set_tokens(settings.tokens);
    }

    pub fn set_tokens(&self, tokens: Vec<WorkerTokenRecord>) {
        let revoked = {
            let mut current = self
                .tokens
                .write()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let revoked = current
                .iter()
                .filter(|old| !tokens.iter().any(|token| token.id == old.id))
                .map(|old| old.id.clone())
                .collect::<Vec<_>>();
            *current = tokens;
            revoked
        };
        for id in revoked {
            self.board.revoke_token(&id);
        }
    }

    pub fn tokens(&self) -> Vec<WorkerTokenRecord> {
        self.tokens
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The id of the token `secret` is, when it is a current worker token.
    pub fn token_id(&self, secret: &str) -> Option<String> {
        if !secret.starts_with(crate::remote_work::protocol::TOKEN_PREFIX) {
            return None;
        }
        let hash = crate::auth_registry::hash_token(secret);
        self.tokens
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .iter()
            .find(|token| tokens::same_hash(&token.token_hash, &hash))
            .map(|token| token.id.clone())
    }

    /// What this server's own worker offers, probing rc-astro and the noise
    /// model again when the last probe is older than a minute. Spawns short-lived processes then.
    fn local_offers(&self) -> Offers {
        let mut cached = self
            .local_offers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((probed, offers, _)) = cached.as_ref()
            && probed.elapsed() < LOCAL_OFFERS_TTL
        {
            return offers.clone();
        }
        let tools = (self.tool_source())();
        // This server stacks its own builds in process; its worker runs
        // the processing steps.
        let (offers, problem) = tools.offers(
            crate::concurrency::logical_cores(),
            &crate::remote_work::WorkChoice::processing_only(),
        );
        if let Some(problem) = &problem {
            tracing::debug!("rc-astro on this server: {problem}");
        }
        *cached = Some((Instant::now(), offers.clone(), problem));
        offers
    }

    /// The last problem reading this server's RC-Astro schemas.
    pub fn local_rc_astro_problem(&self) -> Option<String> {
        self.local_offers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_ref()
            .and_then(|(_, _, problem)| problem.clone())
    }

    fn tool_source(&self) -> ToolSource {
        Arc::clone(
            &self
                .tools
                .read()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }

    /// Use `source` for this server's tools, in place of what is installed.
    #[cfg(test)]
    pub(crate) fn set_tool_source(&self, source: ToolSource) {
        *self
            .tools
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = source;
        self.forget_local_offers();
    }

    /// Probe this server's tools again on the next request, such as after a
    /// download installed one.
    pub fn forget_local_offers(&self) {
        *self
            .local_offers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// Start this server's own worker when it is not running, and bring
    /// what it offers up to date. Spawns short-lived processes when the
    /// offers are a minute old.
    pub fn refresh_local(&self) {
        let offers = self.local_offers();
        let worker = self.local.get_or_init(|| {
            local::LocalWorker::start(Arc::clone(&self.board), offers.clone(), self.tool_source())
        });
        self.board.update_offers(worker.session(), offers);
    }
}

/// The tools installed on this server.
pub(crate) fn local_tools() -> LocalTools {
    LocalTools {
        rc_astro: crate::remote_work::rc_astro::locate(None),
        noise_model: Some(local_noise_model()),
        compute: LOCAL_COMPUTE
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
        yield_to_person: LOCAL_YIELD
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone(),
        this_server: true,
        slots: 1,
    }
}

/// What runs the noise model on this server: Seiza, with the ONNX Runtime
/// path and the GPU choice from Settings as they are now.
fn local_noise_model() -> Arc<dyn crate::remote_work::noise_model::Engine> {
    crate::remote_work::noise_model::SeizaEngine::new(crate::server::noise_model::config())
}

/// Grants this server's worker its threads from the server's shared budget.
static LOCAL_COMPUTE: RwLock<Option<crate::remote_work::ComputeSource>> = RwLock::new(None);

/// How this server's automatic model runs wait for a person's work.
static LOCAL_YIELD: RwLock<Option<crate::remote_work::YieldHook>> = RwLock::new(None);

/// Have this server's automatic model runs wait through `hook` between
/// tiles. A server sets it once it has its state.
pub fn set_local_yield(hook: crate::remote_work::YieldHook) {
    *LOCAL_YIELD
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(hook);
}

/// Lease the threads of this server's noise model runs from `source`. A
/// server sets it once it has its budgets.
pub fn set_local_compute(source: crate::remote_work::ComputeSource) {
    *LOCAL_COMPUTE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(source);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_current_worker_token_is_accepted() {
        let workers = Workers::default();
        let (secret, record) = tokens::mint("studio").unwrap();
        workers.set_tokens(vec![record.clone()]);
        assert_eq!(workers.token_id(&secret), Some(record.id));
        // A user's API token, or anything else, is not a worker token.
        assert_eq!(workers.token_id(&secret.replace("psfw_", "psfg_")), None);
        assert_eq!(workers.token_id("psfw_unknown"), None);
        workers.set_tokens(Vec::new());
        assert_eq!(workers.token_id(&secret), None);
    }
}
