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
        atomic::{AtomicU64, Ordering},
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
}
