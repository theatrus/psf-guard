//! Database-wide, low-priority quality analysis.
//!
//! This job is separate from FITS import and from a user-triggered target
//! scan. It walks targets one at a time, reuses the normal quality cache, and
//! yields between frames whenever interactive work is active.

use serde::Serialize;
use std::sync::{Arc, RwLock};

#[derive(Debug, Clone, Default, Serialize)]
pub struct QualityBackfillProgress {
    pub running: bool,
    pub force: bool,
    pub total_targets: usize,
    pub processed_targets: usize,
    pub current_target_id: Option<i32>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

#[derive(Debug, Default)]
pub struct QualityBackfillStore {
    pub progress: QualityBackfillProgress,
}

pub type SharedQualityBackfill = Arc<RwLock<QualityBackfillStore>>;

pub fn try_begin(store: &RwLock<QualityBackfillStore>, force: bool, total_targets: usize) -> bool {
    let mut state = store.write().unwrap();
    if state.progress.running {
        return false;
    }
    state.progress = QualityBackfillProgress {
        running: total_targets > 0,
        force,
        total_targets,
        started_at: Some(chrono::Utc::now().timestamp()),
        ..Default::default()
    };
    if total_targets == 0 {
        state.progress.finished_at = state.progress.started_at;
    }
    true
}

pub fn begin_target(store: &RwLock<QualityBackfillStore>, target_id: i32) {
    store.write().unwrap().progress.current_target_id = Some(target_id);
}

pub fn finish_target(store: &RwLock<QualityBackfillStore>) {
    let mut state = store.write().unwrap();
    state.progress.processed_targets += 1;
    state.progress.current_target_id = None;
}

pub fn finish(store: &RwLock<QualityBackfillStore>) {
    let mut state = store.write().unwrap();
    state.progress.running = false;
    state.progress.current_target_id = None;
    state.progress.finished_at = Some(chrono::Utc::now().timestamp());
}

pub fn snapshot(store: &RwLock<QualityBackfillStore>) -> QualityBackfillProgress {
    store.read().unwrap().progress.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracks_one_database_wide_job() {
        let store = RwLock::new(QualityBackfillStore::default());
        assert!(try_begin(&store, true, 2));
        assert!(!try_begin(&store, false, 1));
        begin_target(&store, 12);
        finish_target(&store);
        finish(&store);

        let progress = snapshot(&store);
        assert!(!progress.running);
        assert!(progress.force);
        assert_eq!(progress.total_targets, 2);
        assert_eq!(progress.processed_targets, 1);
        assert!(progress.finished_at.is_some());
    }
}
