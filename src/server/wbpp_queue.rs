//! WBPP runs queued for PixInsight.
//!
//! PixInsight uses every core it can get, so the server runs one WBPP run at
//! a time across all databases. Queued runs share one job line, whose worker
//! lane is PixInsight: the line's dispatcher starts the next run when
//! PixInsight is free. While a database copies masters, its queued runs keep
//! their place and runs of other databases start first. The job journal
//! stores the queued runs, so a server restart restores the queue.

use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use serde::{Deserialize, Serialize};

use super::job_line::{
    DispatchRules, Entry, JobLine, KeyHold, Lane, Lease, Task, TaskFuture, Turn, Worker,
};
use super::state::AppState;
use super::wbpp_run::StartWbppRunRequest;
use crate::concurrency::Priority;

/// One run waiting to start. The line stores it as the run's tracked
/// journal record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedRun {
    pub id: String,
    pub db_id: String,
    /// Display label ("project Bubble"), as the run will show it.
    pub scope: String,
    pub project_id: Option<i32>,
    pub target_id: Option<i32>,
    pub queued_at: i64,
    pub request: StartWbppRunRequest,
}

/// What a client sees of a queued run: its place in the whole line, since
/// runs from every database share it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct QueuedRunSummary {
    pub id: String,
    pub scope: String,
    pub project_id: Option<i32>,
    pub target_id: Option<i32>,
    /// 1 is next.
    pub position: usize,
    pub queued_at: i64,
}

impl QueuedRun {
    fn summary(&self, position: usize) -> QueuedRunSummary {
        QueuedRunSummary {
            id: self.id.clone(),
            scope: self.scope.clone(),
            project_id: self.project_id,
            target_id: self.target_id,
            position,
            queued_at: self.queued_at,
        }
    }
}

#[derive(Default)]
pub struct WbppQueue {
    /// The queued runs, in order, with each run's details as its tracked
    /// journal record. PixInsight is the main lane.
    line: JobLine,
    next_id: AtomicU64,
}

/// How the WBPP line runs its entries. PixInsight runs as a separate
/// process, so background work on this server keeps running beside it.
static WBPP_LINE: DispatchRules = DispatchRules {
    name: "WBPP",
    line: |state| &state.wbpp_queue.line,
    lanes: &[Lane::Main],
    turn: |_, _| Turn {
        priority: Priority::Interactive,
        pauses_background_work: false,
    },
};

/// The key a database's masters copy holds: the database's queued runs are
/// blocked by it.
fn database_key(db_id: &str) -> String {
    format!("wbpp-database:{db_id}")
}

impl WbppQueue {
    /// Add a run to the back of the line. Returns its id and 1-based place.
    pub fn push(
        &self,
        state: &Arc<AppState>,
        db_id: &str,
        scope: String,
        request: StartWbppRunRequest,
        now: i64,
    ) -> (String, usize) {
        let id = format!("q{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        self.submit(
            QueuedRun {
                id: id.clone(),
                db_id: db_id.to_string(),
                scope,
                project_id: request.project_id,
                target_id: request.target_id,
                queued_at: now,
                request,
            },
            None,
        );
        self.line.ensure_dispatching(state, &WBPP_LINE);
        let position = self.line.position(&id).map_or(1, |index| index + 1);
        (id, position)
    }

    fn submit(&self, run: QueuedRun, before: Option<&str>) {
        let id = run.id.clone();
        let stop = self.line.track(&id);
        self.line.set_journal(&id, &run);
        let task = Box::new(StartQueuedRun { id: id.clone() });
        let entry =
            Entry::new(&id, Lane::Main, stop, task).blocked_by(vec![database_key(&run.db_id)]);
        if self.line.submit(entry, before).is_err() {
            tracing::warn!(id, "A WBPP run with this id is already queued");
        }
    }

    /// Put a run that could not start yet back at the front of the line,
    /// with its id unchanged.
    pub fn push_front(&self, run: QueuedRun) {
        let first = self.line.order().into_iter().next();
        self.submit(run, first.as_deref());
    }

    /// Reserve PixInsight when it is free and no run is queued, so a run
    /// can start at once. PixInsight is free again when the lease is
    /// dropped.
    pub fn try_reserve_pixinsight(&self) -> Option<Lease> {
        self.line.try_reserve_idle_main()
    }

    /// Reserve PixInsight when it is free, even with runs queued: for a
    /// PixInsight process that outlived a server restart.
    pub fn try_reserve_pixinsight_now(&self) -> Option<Lease> {
        self.line.try_reserve_main()
    }

    /// Block the database's queued runs until the returned hold is dropped:
    /// a run must not start while the database copies masters.
    pub fn hold_database(&self, db_id: &str) -> KeyHold {
        self.line.hold_key(database_key(db_id))
    }

    /// Take the run that the dispatcher starts out of the queue. `None`
    /// when the run was removed meanwhile.
    fn take(&self, id: &str) -> Option<QueuedRun> {
        self.line.take_journal(id)
    }

    /// Take one database's queued run out of the line. `None` when the run
    /// belongs to another database, or already started.
    pub fn remove(&self, db_id: &str, id: &str) -> Option<QueuedRun> {
        let theirs = self
            .line
            .journal_of::<QueuedRun>(id)
            .is_some_and(|run| run.db_id == db_id);
        if !theirs {
            return None;
        }
        // Taken in one step, so the dispatcher cannot also start it.
        let run: QueuedRun = self.line.take_journal(id)?;
        // The dispatcher removes the stopped entry from the line.
        if let Some(stop) = self.line.stop_of(id) {
            stop.request();
            self.line.changed();
        }
        Some(run)
    }

    /// The queued runs, next first.
    fn ordered(&self) -> Vec<QueuedRun> {
        self.line
            .order()
            .iter()
            .filter_map(|id| self.line.journal_of(id))
            .collect()
    }

    /// Whether a database already has a queued run for this project or
    /// target, so a second click does not queue it twice.
    pub fn find_same(
        &self,
        db_id: &str,
        request: &StartWbppRunRequest,
    ) -> Option<QueuedRunSummary> {
        self.summaries_for(db_id).into_iter().find(|entry| {
            entry.project_id == request.project_id && entry.target_id == request.target_id
        })
    }

    /// One database's queued runs with their place in the whole line.
    pub fn summaries_for(&self, db_id: &str) -> Vec<QueuedRunSummary> {
        self.all()
            .into_iter()
            .filter(|(run_db, _)| run_db == db_id)
            .map(|(_, summary)| summary)
            .collect()
    }

    /// Move a queued run to `position` in the line (0 is next), clamped to
    /// the line. False when no such run is waiting.
    pub fn move_to(&self, id: &str, position: usize) -> bool {
        self.line.stop_flag(id).is_some() && self.line.move_to(id, position)
    }

    /// Every queued run, with its database and place in the line.
    pub fn all(&self) -> Vec<(String, QueuedRunSummary)> {
        self.ordered()
            .into_iter()
            .enumerate()
            .map(|(index, run)| (run.db_id.clone(), run.summary(index + 1)))
            .collect()
    }

    /// Every queued run with its request, next first, for the job journal.
    pub fn journal_entries(&self) -> Vec<QueuedRun> {
        self.ordered()
    }

    pub fn len(&self) -> usize {
        self.line.journals::<QueuedRun>().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A queued run's entry in the line: starts the run when PixInsight is free.
struct StartQueuedRun {
    id: String,
}

impl Task for StartQueuedRun {
    fn what(&self) -> &'static str {
        "a WBPP run"
    }

    /// The dispatcher takes the future instead; this runs the same future
    /// on the calling thread for any other caller.
    fn run(mut self: Box<Self>, state: &Arc<AppState>, _worker: &Worker<'_>) {
        if let Some(work) = self.take_future(state) {
            tokio::runtime::Handle::current().block_on(work);
        }
    }

    /// The WBPP run as a future, which ends when the run ends, so
    /// PixInsight stays reserved for the whole run. `None`, and nothing
    /// runs, when the run was removed from the queue meanwhile.
    fn take_future(&mut self, state: &Arc<AppState>) -> Option<TaskFuture> {
        let state = Arc::clone(state);
        let id = std::mem::take(&mut self.id);
        Some(Box::pin(async move {
            if let Some(run) = state.wbpp_queue.take(&id) {
                super::wbpp_run::start_queued(state, run).await;
            }
        }))
    }

    fn stopped(self: Box<Self>, state: &Arc<AppState>) {
        state.wbpp_queue.take(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(project_id: i32) -> StartWbppRunRequest {
        StartWbppRunRequest {
            project_id: Some(project_id),
            ..Default::default()
        }
    }

    fn test_state() -> Arc<AppState> {
        Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ))
    }

    #[tokio::test]
    async fn the_line_is_shared_across_databases_and_positions_are_global() {
        let state = test_state();
        // PixInsight is busy, so nothing starts.
        let _pixinsight = state.wbpp_queue.try_reserve_pixinsight().unwrap();
        let queue = &state.wbpp_queue;
        let (a, first) = queue.push(&state, "alpha", "Sh2 86".into(), request(1), 10);
        let (b, second) = queue.push(&state, "beta", "M31".into(), request(7), 11);
        let (_c, third) = queue.push(&state, "alpha", "NGC 6820".into(), request(2), 12);
        assert_eq!((first, second, third), (1, 2, 3));
        let alpha = queue.summaries_for("alpha");
        assert_eq!(alpha.len(), 2);
        assert_eq!((alpha[0].position, alpha[1].position), (1, 3));
        assert_eq!(alpha[1].scope, "NGC 6820");
        assert!(queue.find_same("alpha", &request(2)).is_some());
        assert!(
            queue.find_same("alpha", &request(7)).is_none(),
            "beta's run"
        );

        assert!(
            queue.remove("beta", &a).is_none(),
            "an id belongs to its database"
        );
        assert_eq!(
            queue.remove("beta", &b).map(|run| run.scope),
            Some("M31".into())
        );
        assert_eq!(
            queue.summaries_for("alpha")[1].position,
            2,
            "the line closes up"
        );
        assert_eq!(queue.all()[0].1.id, a);
        assert_eq!(queue.len(), 2);
    }

    #[tokio::test]
    async fn a_queued_run_moves_within_the_line() {
        let state = test_state();
        let _pixinsight = state.wbpp_queue.try_reserve_pixinsight().unwrap();
        let queue = &state.wbpp_queue;
        let (first, _) = queue.push(&state, "a", "project 1".into(), request(1), 1);
        let (second, _) = queue.push(&state, "b", "project 2".into(), request(2), 2);
        let (third, _) = queue.push(&state, "a", "project 3".into(), request(3), 3);
        assert!(queue.move_to(&third, 0));
        let order = queue
            .all()
            .into_iter()
            .map(|(_, run)| run.id)
            .collect::<Vec<_>>();
        assert_eq!(order, vec![third.clone(), first.clone(), second.clone()]);
        // Past the end goes last; an unknown id moves nothing.
        assert!(queue.move_to(&third, 99));
        assert_eq!(queue.all().last().unwrap().1.id, third);
        assert!(!queue.move_to("q99", 0));
        assert_eq!(queue.all()[0].0, "a");
        assert_eq!(queue.all()[0].1.position, 1);
    }

    #[tokio::test]
    async fn pixinsight_is_reserved_once_and_a_run_goes_back_to_the_front() {
        let state = test_state();
        let queue = &state.wbpp_queue;
        let pixinsight = queue.try_reserve_pixinsight().unwrap();
        assert!(
            queue.try_reserve_pixinsight().is_none(),
            "a second start waits"
        );
        drop(pixinsight);
        let _pixinsight = queue.try_reserve_pixinsight().unwrap();

        let (first, _) = queue.push(&state, "a", "project 1".into(), request(1), 1);
        let (second, _) = queue.push(&state, "a", "project 2".into(), request(2), 2);
        let taken = queue.take(&first).unwrap();
        queue.line.remove(&first);
        queue.push_front(taken);
        let order = queue
            .all()
            .into_iter()
            .map(|(_, run)| run.id)
            .collect::<Vec<_>>();
        assert_eq!(order, vec![first, second]);
    }

    #[tokio::test]
    async fn a_database_copying_masters_blocks_only_its_own_runs() {
        let state = test_state();
        let queue = &state.wbpp_queue;
        let copying = queue.hold_database("a");
        let pixinsight = queue.try_reserve_pixinsight().unwrap();
        let (blocked, _) = queue.push(&state, "a", "project 1".into(), request(1), 1);
        let (other, _) = queue.push(&state, "b", "project 2".into(), request(2), 2);
        drop(pixinsight);
        // The dispatcher passes over a's run and starts b's, which fails at
        // once: the test server has no PixInsight.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while queue.line.stop_flag(&other).is_some() {
            assert!(
                std::time::Instant::now() < deadline,
                "b's run never started"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(queue.all()[0].1.id, blocked, "a's run keeps its place");
        drop(copying);
    }
}
