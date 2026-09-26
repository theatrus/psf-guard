//! Automatic import of new frames.
//!
//! A database whose entry carries [`AutoImportSettings`] gets its image
//! folders scanned on its own: once when the server or desktop app opens it,
//! and again on a schedule. A run is the ordinary import job with
//! `only_new` set, so files the catalog already holds cost a directory walk
//! and one query rather than a header read each, and the matching rules,
//! preview shapes and job progress are the ones the Import button uses.
//!
//! One scheduler loop serves every database, re-reading the open set each
//! tick, so a database added or edited while the server runs joins without
//! a restart. A run never starts while an import or a remote upload holds the
//! database's import lock, or while the user has an interactive job running;
//! it waits for the next tick.

use crate::commands::import::ImportOptions;
use crate::db_registry::AutoImportSettings;
use crate::server::database_context::DatabaseContext;
use crate::server::state::AppState;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the scheduler looks for due runs.
pub const TICK: Duration = Duration::from_secs(30);

/// What the scheduler remembers about one database.
#[derive(Debug, Clone)]
pub struct Record {
    /// When the scheduler first saw the database, or when its last run
    /// started. Schedules count from here.
    anchor: Instant,
    anchor_unix: i64,
    /// Whether an on-open run (or the decision not to have one) happened.
    opened: bool,
    last_started_unix: Option<i64>,
}

/// Why a run starts now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Open,
    Schedule,
    Manual,
}

#[derive(Default)]
pub struct AutoImportScheduler {
    records: Mutex<HashMap<String, Record>>,
}

/// What `GET /api/db/{db_id}/autoimport` reports.
#[derive(Debug, Clone, Serialize)]
pub struct AutoImportStatus {
    pub settings: Option<AutoImportSettings>,
    /// Unix seconds of the last automatic run's start, if any this process.
    pub last_started_at: Option<i64>,
    /// Unix seconds when the next scheduled run is due, when a schedule is
    /// set and the database is enabled.
    pub next_run_at: Option<i64>,
    /// The import job's state when its last run was automatic; the UI
    /// describes it the way it describes a manual import.
    pub progress: Option<crate::server::import_job::ImportJobProgress>,
}

/// Decide whether a run is due, given the settings and what the scheduler
/// remembers. Pure, so the schedule logic is testable without a clock.
pub fn due(settings: &AutoImportSettings, record: Option<&Record>, now: Instant) -> Option<Reason> {
    if !settings.enabled {
        return None;
    }
    let Some(record) = record else {
        // First sight of this database: on open, or start the schedule
        // counting from now.
        return settings.on_open.then_some(Reason::Open);
    };
    if !record.opened && settings.on_open {
        return Some(Reason::Open);
    }
    if settings.interval_minutes > 0
        && now.duration_since(record.anchor)
            >= Duration::from_secs(u64::from(settings.interval_minutes) * 60)
    {
        return Some(Reason::Schedule);
    }
    None
}

impl AutoImportScheduler {
    fn note_seen(&self, database_id: &str, now: Instant) {
        let mut records = self.records.lock().unwrap();
        records.entry(database_id.to_string()).or_insert(Record {
            anchor: now,
            anchor_unix: chrono::Utc::now().timestamp(),
            opened: true,
            last_started_unix: None,
        });
    }

    fn note_started(&self, database_id: &str, now: Instant) {
        let unix = chrono::Utc::now().timestamp();
        let mut records = self.records.lock().unwrap();
        let record = records.entry(database_id.to_string()).or_insert(Record {
            anchor: now,
            anchor_unix: unix,
            opened: true,
            last_started_unix: None,
        });
        record.anchor = now;
        record.anchor_unix = unix;
        record.opened = true;
        record.last_started_unix = Some(unix);
    }

    /// Forget databases that are no longer open.
    fn retain(&self, ids: &[String]) {
        self.records
            .lock()
            .unwrap()
            .retain(|id, _| ids.iter().any(|open| open == id));
    }

    pub fn status(&self, ctx: &DatabaseContext) -> AutoImportStatus {
        let record = self.records.lock().unwrap().get(&ctx.id).cloned();
        let settings = ctx.autoimport.clone();
        let next_run_at = match (&settings, &record) {
            (Some(settings), Some(record)) if settings.enabled && settings.interval_minutes > 0 => {
                Some(record.anchor_unix + i64::from(settings.interval_minutes) * 60)
            }
            _ => None,
        };
        let progress = crate::server::import_job::progress_snapshot(&ctx.import_job);
        AutoImportStatus {
            settings,
            last_started_at: record.and_then(|record| record.last_started_unix),
            next_run_at,
            progress: (progress.trigger == "automatic").then_some(progress),
        }
    }
}

/// Run the scheduler for the life of the process.
pub fn spawn(state: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(TICK).await;
            tick(&state, Instant::now()).await;
        }
    })
}

/// One pass over the open databases. `now` is a parameter so a test can
/// move the clock.
pub async fn tick(state: &Arc<AppState>, now: Instant) {
    let databases = state.all_databases();
    let ids = databases
        .iter()
        .map(|ctx| ctx.id.clone())
        .collect::<Vec<_>>();
    state.autoimport.retain(&ids);
    for ctx in databases {
        let Some(settings) = ctx.autoimport.clone() else {
            continue;
        };
        let reason = {
            let records = state.autoimport.records.lock().unwrap();
            due(&settings, records.get(&ctx.id), now)
        };
        let Some(reason) = reason else {
            // A schedule counts from the first tick that saw it enabled. A
            // database turned on later is "first seen" then, so an on-open
            // run follows within a tick of enabling it.
            if settings.enabled {
                state.autoimport.note_seen(&ctx.id, now);
            }
            continue;
        };
        if state.interactive_job_active() {
            tracing::debug!(
                "⏸️ Automatic import for db={} waits: interactive job running",
                ctx.id
            );
            continue;
        }
        match start(state, &ctx, &settings, reason, now) {
            Ok(true) => {}
            Ok(false) => tracing::debug!(
                "⏸️ Automatic import for db={} waits: an import is already running",
                ctx.id
            ),
            Err(error) => tracing::warn!(
                "📥 Automatic import for db={} did not start: {error:#}",
                ctx.id
            ),
        }
    }
}

/// Start one run now. Returns `Ok(false)` when the database's import job or
/// import lock is busy.
pub fn start(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    settings: &AutoImportSettings,
    reason: Reason,
    now: Instant,
) -> anyhow::Result<bool> {
    if ctx.image_dirs.is_empty() {
        anyhow::bail!("the database has no image directories");
    }
    let missing = ctx
        .image_dirs
        .iter()
        .filter(|dir| !std::path::Path::new(dir).is_dir())
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        anyhow::bail!("image directory not found: {}", missing.join(", "));
    }
    // A remote upload in flight holds this lock; let it finish rather than
    // queue behind it and stall the next tick's decisions.
    if ctx.image_import_mutex.try_lock().is_err() {
        return Ok(false);
    }
    let options = ImportOptions {
        scope: settings.scope,
        accept_other_rigs: settings.accept_other_rigs,
        only_new: true,
        ..ImportOptions::default()
    };
    let started = crate::server::handlers::spawn_import_job_with_trigger(
        state,
        Arc::clone(ctx),
        ctx.image_dirs.clone(),
        options,
        settings.backfill,
        true,
        // "Run now" is the automatic import by hand: it shows up on the same
        // status line, with the same settings.
        "automatic",
    );
    if started {
        tracing::info!(
            "📥 Automatic import started for db={} ({})",
            ctx.id,
            match reason {
                Reason::Open => "on open",
                Reason::Schedule => "scheduled",
                Reason::Manual => "run now",
            }
        );
        state.autoimport.note_started(&ctx.id, now);
    }
    Ok(started)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(enabled: bool, on_open: bool, interval_minutes: u32) -> AutoImportSettings {
        AutoImportSettings {
            enabled,
            on_open,
            interval_minutes,
            ..AutoImportSettings::default()
        }
    }

    fn record(anchor: Instant, opened: bool) -> Record {
        Record {
            anchor,
            anchor_unix: 0,
            opened,
            last_started_unix: None,
        }
    }

    #[test]
    fn disabled_never_runs() {
        let now = Instant::now();
        assert_eq!(due(&settings(false, true, 5), None, now), None);
        assert_eq!(
            due(
                &settings(false, true, 5),
                Some(&record(now - Duration::from_secs(3600), true)),
                now
            ),
            None
        );
    }

    #[test]
    fn first_sight_runs_on_open_only_when_asked() {
        let now = Instant::now();
        assert_eq!(due(&settings(true, true, 0), None, now), Some(Reason::Open));
        assert_eq!(due(&settings(true, false, 30), None, now), None);
    }

    #[test]
    fn schedule_counts_from_the_anchor() {
        let now = Instant::now();
        let fresh = record(now, true);
        assert_eq!(due(&settings(true, true, 30), Some(&fresh), now), None);
        let due_record = record(now - Duration::from_secs(31 * 60), true);
        assert_eq!(
            due(&settings(true, true, 30), Some(&due_record), now),
            Some(Reason::Schedule)
        );
        assert_eq!(
            due(&settings(true, true, 0), Some(&due_record), now),
            None,
            "no schedule means only on open"
        );
    }

    #[test]
    fn enabling_on_open_later_runs_once() {
        let now = Instant::now();
        let seen = record(now, false);
        assert_eq!(
            due(&settings(true, true, 0), Some(&seen), now),
            Some(Reason::Open)
        );
    }

    #[test]
    fn scheduler_records_starts_and_forgets_closed_databases() {
        let scheduler = AutoImportScheduler::default();
        let now = Instant::now();
        scheduler.note_seen("a", now);
        scheduler.note_started("b", now);
        {
            let records = scheduler.records.lock().unwrap();
            assert!(records["a"].last_started_unix.is_none());
            assert!(records["b"].last_started_unix.is_some());
        }
        scheduler.retain(&["b".to_string()]);
        assert!(!scheduler.records.lock().unwrap().contains_key("a"));
    }
}
