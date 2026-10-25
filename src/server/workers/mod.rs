//! Workers: machines that run tasks for this server. See
//! `docs/design/remote-workers.md`.
//!
//! A worker joins with a token minted in Settings › Workers, registers what
//! it can run, and claims tasks from the [`board`]. This server runs its own
//! worker in process (`local`), which claims tasks through the same board,
//! so every task this server runs takes the path a remote one does.

pub mod board;
mod local;
pub mod rc_astro;
pub mod routes;
pub mod tokens;

use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::db_registry::WorkerTokenRecord;
use crate::remote_work::protocol::Offers;
use crate::remote_work::LocalTools;

pub use board::{Board, Finished, NewTask, TaskNeeds, Ticket, WaitError};

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
}

impl Default for Workers {
    fn default() -> Self {
        Self {
            board: Arc::new(Board::default()),
            tokens: RwLock::new(Vec::new()),
            local: OnceLock::new(),
            local_offers: Mutex::new(None),
        }
    }
}

#[cfg(test)]
impl Workers {
    /// Workers on `board`, such as one with short leases.
    pub(crate) fn with_board(board: Board) -> Self {
        Self {
            board: Arc::new(board),
            tokens: RwLock::new(Vec::new()),
            local: OnceLock::new(),
            local_offers: Mutex::new(None),
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
    /// Take the tokens and the exchange folder from the registry.
    pub fn configure(&self, tokens: Vec<WorkerTokenRecord>, exchange: std::path::PathBuf) {
        // Files of tasks from an earlier run of the server are of no use.
        for stale in ["tasks", "probes"] {
            let _ = std::fs::remove_dir_all(exchange.join(stale));
        }
        self.board.set_exchange(exchange);
        self.set_tokens(tokens);
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

    /// What this server's own worker offers, probing rc-astro again when the
    /// last probe is older than a minute. Spawns short-lived processes then.
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
        let tools = local_tools();
        let (offers, problem) = tools.offers(
            crate::concurrency::logical_cores(),
            &crate::remote_work::WorkChoice::default(),
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

    /// Start this server's own worker when it is not running, and bring
    /// what it offers up to date. Spawns short-lived processes when the
    /// offers are a minute old.
    pub fn refresh_local(&self) {
        let offers = self.local_offers();
        let worker = self
            .local
            .get_or_init(|| local::LocalWorker::start(Arc::clone(&self.board), offers.clone()));
        self.board.update_offers(worker.session(), offers);
    }
}

/// The tools installed on this server.
pub(crate) fn local_tools() -> LocalTools {
    LocalTools {
        rc_astro: crate::remote_work::rc_astro::locate(None),
    }
}
