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

/// What waits for the database's runner: each refresh with the context
/// that asked for it, which marked a files refresh started.
#[derive(Default)]
pub(crate) struct Queue {
    files: Option<DatabaseContext>,
    tree: Option<DatabaseContext>,
    /// The context of the files refresh under way, which a failed runner
    /// marks finished.
    running_files: Option<DatabaseContext>,
}

/// A refresh to ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refresh {
    Files,
    Tree,
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
        queue.running_files = None;
        if let Some(ctx) = queue.files.take() {
            queue.running_files = Some(ctx.clone());
            return Step::Run(Item::Files(ctx));
        }
        match queue.tree.take() {
            Some(ctx) => Step::Run(Item::Tree(ctx)),
            None => Step::Exit,
        }
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
        queue.files.is_some() || queue.tree.is_some()
    }

    fn runner_failed(queue: &mut Queue, _: &mut ()) {
        // A files refresh that panicked or never ran still has to end, or
        // the database would report a refresh under way for good.
        for ctx in [queue.running_files.take(), queue.files.take()]
            .into_iter()
            .flatten()
        {
            ctx.file_check_cache
                .write()
                .unwrap()
                .mark_refresh_completed();
        }
        queue.tree = None;
    }
}

/// Ask the database's runner for `refresh` of `ctx`. A files refresh
/// replacing one of another context still waiting ends that one's mark,
/// since it will not run.
pub(crate) fn request(state: &Arc<AppState>, ctx: &DatabaseContext, refresh: Refresh) {
    state
        .catalog_jobs
        .catalog_refresh
        .request(state, &ctx.id, |queue| match refresh {
            Refresh::Files => {
                if let Some(replaced) = queue.files.replace(ctx.clone())
                    && !Arc::ptr_eq(&replaced.file_check_cache, &ctx.file_check_cache)
                {
                    replaced
                        .file_check_cache
                        .write()
                        .unwrap()
                        .mark_refresh_completed();
                }
            }
            Refresh::Tree => queue.tree = Some(ctx.clone()),
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(ctx: &DatabaseContext) -> bool {
        ctx.get_cache_refresh_progress().is_some()
    }

    #[test]
    fn each_refresh_runs_on_the_context_that_asked_and_a_failure_ends_it() {
        let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        let old = DatabaseContext::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        let new = DatabaseContext::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        new.file_check_cache.write().unwrap().mark_refresh_started();
        let mut queue = Queue {
            files: Some(new.clone()),
            tree: Some(old.clone()),
            running_files: None,
        };
        let next = |queue: &mut Queue| CatalogRefresh::next(&state, "db", queue, Instant::now());
        match next(&mut queue) {
            Step::Run(Item::Files(ctx)) => {
                assert!(Arc::ptr_eq(&ctx.file_check_cache, &new.file_check_cache))
            }
            _ => panic!("the files refresh runs first"),
        }
        // The runner fails during it: the new context's refresh ends.
        CatalogRefresh::runner_failed(&mut queue, &mut ());
        assert!(!started(&new));
        assert!(queue.tree.is_none() && !CatalogRefresh::has_work(&queue));
    }

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
