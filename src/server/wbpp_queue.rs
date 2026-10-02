//! WBPP runs waiting their turn.
//!
//! PixInsight takes every core it can get, so the server runs one WBPP job at
//! a time across all databases. A start that arrives while one runs joins
//! this queue in order and starts on its own when the running job ends,
//! whatever its outcome. The queue lives in memory: a server restart drops
//! it, as it drops the run under way.

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
};

use serde::Serialize;

use super::wbpp_run::StartWbppRunRequest;

/// One run waiting to start.
#[derive(Debug, Clone)]
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

#[derive(Debug, Default)]
pub struct WbppQueue {
    entries: Mutex<VecDeque<QueuedRun>>,
    next_id: AtomicU64,
    /// PixInsight's one slot, server-wide. Claimed in one step before a run
    /// starts and released when it ends, so two starts that arrive together,
    /// or a start racing the line, cannot both launch PixInsight.
    slot: AtomicBool,
}

impl WbppQueue {
    /// Add a run to the back of the line. Returns its id and 1-based place.
    pub fn push(
        &self,
        db_id: &str,
        scope: String,
        request: StartWbppRunRequest,
        now: i64,
    ) -> (String, usize) {
        let id = format!("q{}", self.next_id.fetch_add(1, Ordering::Relaxed) + 1);
        let mut entries = self.entries.lock().unwrap();
        entries.push_back(QueuedRun {
            id: id.clone(),
            db_id: db_id.to_string(),
            scope,
            project_id: request.project_id,
            target_id: request.target_id,
            queued_at: now,
            request,
        });
        (id, entries.len())
    }

    /// The next run to start, removed from the line.
    pub fn pop_front(&self) -> Option<QueuedRun> {
        self.entries.lock().unwrap().pop_front()
    }

    /// Put a run that could not start yet back at the head of the line,
    /// with its id and place unchanged.
    pub fn push_front(&self, run: QueuedRun) {
        self.entries.lock().unwrap().push_front(run);
    }

    /// Claim PixInsight's slot. False when a run already holds it.
    pub fn try_claim(&self) -> bool {
        self.slot
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// Give the slot back: the run ended, or a claimed start did not launch.
    pub fn release(&self) {
        self.slot.store(false, Ordering::Release);
    }

    /// Take one database's queued run out of the line.
    pub fn remove(&self, db_id: &str, id: &str) -> Option<QueuedRun> {
        let mut entries = self.entries.lock().unwrap();
        let index = entries
            .iter()
            .position(|entry| entry.db_id == db_id && entry.id == id)?;
        entries.remove(index)
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
        self.entries
            .lock()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.db_id == db_id)
            .map(|(index, entry)| QueuedRunSummary {
                id: entry.id.clone(),
                scope: entry.scope.clone(),
                project_id: entry.project_id,
                target_id: entry.target_id,
                position: index + 1,
                queued_at: entry.queued_at,
            })
            .collect()
    }

    /// Move a queued run to `position` in the line (0 is next), clamped to
    /// the line. False when no such run is waiting.
    pub fn move_to(&self, id: &str, position: usize) -> bool {
        let mut entries = self.entries.lock().unwrap();
        let Some(index) = entries.iter().position(|entry| entry.id == id) else {
            return false;
        };
        let entry = entries.remove(index).expect("index came from the line");
        let position = position.min(entries.len());
        entries.insert(position, entry);
        true
    }

    /// Every queued run, with its database and place in the line.
    pub fn all(&self) -> Vec<(String, QueuedRunSummary)> {
        self.entries
            .lock()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                (
                    entry.db_id.clone(),
                    QueuedRunSummary {
                        id: entry.id.clone(),
                        scope: entry.scope.clone(),
                        project_id: entry.project_id,
                        target_id: entry.target_id,
                        position: index + 1,
                        queued_at: entry.queued_at,
                    },
                )
            })
            .collect()
    }

    /// Every queued run with its request, next first, for the job journal.
    pub fn journal_entries(&self) -> Vec<QueuedRun> {
        self.entries.lock().unwrap().iter().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
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

    #[test]
    fn the_line_is_shared_across_databases_and_positions_are_global() {
        let queue = WbppQueue::default();
        let (a, first) = queue.push("alpha", "Sh2 86".into(), request(1), 10);
        let (b, second) = queue.push("beta", "M31".into(), request(7), 11);
        let (_c, third) = queue.push("alpha", "NGC 6820".into(), request(2), 12);
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
        assert_eq!(queue.pop_front().map(|run| run.id), Some(a));
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn a_queued_run_moves_within_the_line() {
        let queue = WbppQueue::default();
        let (first, _) = queue.push("a", "project 1".into(), request(1), 1);
        let (second, _) = queue.push("b", "project 2".into(), request(2), 2);
        let (third, _) = queue.push("a", "project 3".into(), request(3), 3);
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

    #[test]
    fn the_slot_is_claimed_once_and_a_run_goes_back_to_the_head() {
        let queue = WbppQueue::default();
        assert!(queue.try_claim());
        assert!(!queue.try_claim(), "a second start waits");
        queue.release();
        assert!(queue.try_claim());

        let (first, _) = queue.push("a", "project 1".into(), request(1), 1);
        let (second, _) = queue.push("a", "project 2".into(), request(2), 2);
        let popped = queue.pop_front().unwrap();
        assert_eq!(popped.id, first);
        queue.push_front(popped);
        let order = queue
            .all()
            .into_iter()
            .map(|(_, run)| run.id)
            .collect::<Vec<_>>();
        assert_eq!(order, vec![first, second]);
    }
}
