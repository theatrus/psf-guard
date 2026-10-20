//! Refreshes of a database's file caches, on one [`CatalogJobs`] runner per
//! database:
//!
//! - **Files.** The cache refresh that finds each frame's file, which the
//!   header lists as the database's refresh.
//! - **Directory tree.** A stale folder index rebuilt while requests use
//!   the stale one.
//!
//! A request while the same refresh is queued adds nothing; one asked for
//! while it runs runs once more after it.
//!
//! [`CatalogJobs`]: crate::server::job_line::catalog::CatalogJobs

use std::sync::Arc;
use std::time::Instant;

use crate::concurrency::Stop;
use crate::server::database_context::DatabaseContext;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Step};
use crate::server::state::AppState;

/// A database's cache refreshes as a job with one runner per database.
pub(crate) struct CatalogRefresh;

/// What waits for the database's runner.
#[derive(Default)]
pub(crate) struct Queue {
    /// The newest context of the database, which the refreshes work on.
    ctx: Option<DatabaseContext>,
    pub files: bool,
    pub tree: bool,
}

/// One refresh, with the context it works on.
pub(crate) enum Item {
    Files(DatabaseContext),
    Tree(DatabaseContext),
}

impl CatalogWork for CatalogRefresh {
    type Queue = Queue;
    type Status = ();
    type Item = Item;
    const RUNNER: &'static str = "catalog-refresh";
    /// A refresh only reads storage and fills caches.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = None;

    fn next(_: &AppState, _: &str, queue: &mut Queue, _: Instant) -> Step<Item> {
        let Some(ctx) = queue.ctx.clone() else {
            return Step::Exit;
        };
        if std::mem::take(&mut queue.files) {
            return Step::Run(Item::Files(ctx));
        }
        if std::mem::take(&mut queue.tree) {
            return Step::Run(Item::Tree(ctx));
        }
        queue.ctx = None;
        Step::Exit
    }

    fn run(_: &Arc<AppState>, _: &str, _: &CatalogJob<Self>, item: Item, _: &Stop) {
        match item {
            Item::Files(ctx) => match tokio::runtime::Handle::try_current() {
                Ok(runtime) => runtime.block_on(ctx.run_cache_refresh()),
                Err(_) => {
                    tracing::warn!("No async runtime to refresh the caches of db={} on", ctx.id);
                    ctx.file_check_cache
                        .write()
                        .unwrap()
                        .mark_refresh_completed();
                }
            },
            Item::Tree(ctx) => ctx.revalidate_directory_tree(),
        }
    }

    fn has_work(queue: &Queue) -> bool {
        queue.files || queue.tree
    }

    fn runner_failed(queue: &mut Queue, _: &mut ()) {
        // A files refresh that panicked still has to end, or the database
        // would report a refresh under way for good.
        if let Some(ctx) = queue.ctx.take() {
            ctx.file_check_cache
                .write()
                .unwrap()
                .mark_refresh_completed();
        }
        queue.files = false;
        queue.tree = false;
    }
}

/// Ask the database's runner for a refresh, with `ctx` as the context it
/// works on.
pub(crate) fn request(state: &Arc<AppState>, ctx: &DatabaseContext, ask: impl FnOnce(&mut Queue)) {
    state
        .catalog_jobs
        .catalog_refresh
        .request(state, &ctx.id, |queue| {
            queue.ctx = Some(ctx.clone());
            ask(queue);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_linked_database_refreshes_its_caches_on_its_runner() {
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        let ctx = state.all_databases().remove(0);
        state.link_contexts(&ctx.database_path);
        let _ = ctx.ensure_cache_available();
        assert!(
            state
                .catalog_jobs
                .catalog_refresh
                .existing(&ctx.id)
                .is_some(),
            "the refresh ran on the database's runner"
        );
        let started = Instant::now();
        while ctx.get_cache_refresh_progress().is_some() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "the refresh never ended"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }
}
