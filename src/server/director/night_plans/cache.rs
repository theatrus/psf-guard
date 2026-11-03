//! Night plans on the job line, and the views kept of them.
//!
//! Every computation runs on the job line, as a `night_plans` run of
//! `AppState.previews`, which the header lists with Stop. One a person
//! waits for holds the interactive guard, so background work steps aside;
//! one the server starts on its own, to keep a view current, does not. A
//! newer request for the same edited night stops the one before it.
//!
//! A request without unsaved edits or fixed instants is kept, by what it
//! asks for, with what it was computed from ([`Inputs`]): the rows written
//! to the meta store, leaving out a rig's status and contact records, and
//! each rig database it planned from, as the registry names it and as its
//! file last changed. The same request again gets the kept view at once
//! while those are unchanged. When one changed, the request waits for a
//! computation that read them after it did, since someone usually just
//! changed them.
//!
//! A view ages without any change: in [`UNDER_WAY_MS`] when it plans the
//! night under way, which is planned from now; when its first night
//! begins; when a forecast it used expires. An aged view is answered at
//! once, marked `refreshing`, while a newer one is computed, unless it is
//! more than [`STALE_MS`] old or its first night has ended: then the
//! request waits. Requests for one view share one computation, on a task
//! of its own. A timer keeps the views read in the last [`VIEWED_MS`]
//! current. Stop ends a computation for everyone who waits for it: they
//! get the kept view, and automatic work leaves it alone for a while.

use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::watch;

/// How long a view that plans the night under way stands.
const UNDER_WAY_MS: u64 = 2 * 60_000;
/// An aged view older than this is computed again before it is answered.
const STALE_MS: u64 = 10 * 60_000;
/// Views read within this long are kept current by the timer.
const VIEWED_MS: u64 = 10 * 60_000;
/// Views not read for this long are let go.
const KEPT_MS: u64 = 60 * 60_000;
/// The most views kept at once; the least recently read go first.
const MAX_KEPT: usize = 32;
/// How often the timer looks for views to compute again.
pub(in crate::server::director) const REFRESH_EVERY: std::time::Duration =
    std::time::Duration::from_secs(60);
const DAY_MS: u64 = 86_400_000;

type Outcome = Result<Kept, NightPlanError>;

/// The views kept, and the computations under way.
#[derive(Default)]
pub(in crate::server::director) struct Cache {
    entries: Mutex<HashMap<String, Entry>>,
    flights: Mutex<HashMap<String, Flight>>,
    /// Numbers each reading of the inputs, so a request can tell a
    /// computation that read them after it did.
    readings: AtomicU64,
    /// Numbers the runs of requests that are not kept, so none replaces
    /// another.
    next: AtomicU64,
    /// Holds each computation, once computed and before it is kept, until a
    /// permit is added.
    #[cfg(test)]
    hold: Mutex<Option<Arc<tokio::sync::Semaphore>>>,
}

struct Entry {
    request: Request,
    kept: Kept,
    inputs: Inputs,
    /// The rig databases the view planned from.
    catalogs: Vec<String>,
    /// When the view no longer stands, though nothing it was computed from
    /// changed.
    ages_at_ms: Option<u64>,
    read_at_ms: u64,
    /// Until when automatic work leaves the view alone, after Stop.
    paused_until_ms: u64,
}

/// A computation under way, and the reading of the inputs it starts from.
struct Flight {
    receiver: watch::Receiver<Option<Outcome>>,
    reading: u64,
}

/// A view, when it was computed, and whether it is being computed again.
#[derive(Clone)]
pub(super) struct Kept {
    pub(super) view: Arc<View>,
    pub(super) computed_at_ms: u64,
    pub(super) refreshing: bool,
    /// The reading of the inputs it was computed from.
    reading: u64,
}

/// Who waits for a computation.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Waiting {
    /// A person, or an agent: the computation holds the interactive guard.
    Person,
    /// No one: the server keeps a view current.
    Automatic,
}

/// What views are computed from.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Inputs {
    /// [`MetaStore::planning_rows_written`] after the last write.
    meta: u64,
    /// Each rig database, by its registry id.
    catalogs: BTreeMap<String, Catalog>,
    database_management: bool,
}

/// What a view takes from a registered rig database.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Catalog {
    name: String,
    /// Target Scheduler's own file, for a catalog kept in step with it.
    ts_source: Option<String>,
    /// When the database and its write-ahead log last changed, and their
    /// sizes.
    files: Vec<Option<(SystemTime, u64)>>,
}

impl Inputs {
    /// The inputs now, with every registered rig database.
    fn read(state: &AppState, service: &Service) -> Self {
        let catalogs = state
            .databases
            .read()
            .map(|databases| {
                databases
                    .values()
                    .map(|catalog| {
                        let path = &catalog.database_path;
                        let files = [format!("{path}-wal"), path.clone()]
                            .into_iter()
                            .map(|file| {
                                // A reader opening the database creates an
                                // empty write-ahead log, which holds no
                                // change.
                                let wal = file.ends_with("-wal");
                                std::fs::metadata(&file)
                                    .ok()
                                    .filter(|meta| !(wal && meta.len() == 0))
                                    .and_then(|meta| Some((meta.modified().ok()?, meta.len())))
                            })
                            .collect();
                        (
                            catalog.id.clone(),
                            Catalog {
                                name: catalog.name.clone(),
                                ts_source: catalog
                                    .ts_sync
                                    .as_ref()
                                    .map(|sync| sync.source_path.clone()),
                                files,
                            },
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            meta: service.planning_rows.load(Ordering::Acquire),
            catalogs,
            database_management: state.database_management_allowed(),
        }
    }

    /// These inputs with only the rig databases a view planned from.
    fn of(&self, catalogs: &[String]) -> Self {
        Self {
            meta: self.meta,
            catalogs: self
                .catalogs
                .iter()
                .filter(|(id, _)| catalogs.contains(id))
                .map(|(id, catalog)| (id.clone(), catalog.clone()))
                .collect(),
            database_management: self.database_management,
        }
    }
}

/// The key a request's view is kept by, or `None` for a request that is
/// computed each time: one with unsaved edits, or with fixed instants.
fn key(request: &Request) -> Option<String> {
    if request.override_draft.is_some()
        || request.ts_draft.is_some()
        || request.start_ms.is_some()
        || request.plan_from_ms.is_some()
    {
        return None;
    }
    let mut rigs = request.rigs.clone().unwrap_or_default();
    rigs.sort();
    rigs.dedup();
    Some(format!(
        "{rigs:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
        request.first_night,
        request.nights,
        request.schedule_nights,
        request.explain,
        request.runtime,
        request.fidelity,
        request.utc_offset_minutes,
    ))
}

/// What the header names a run.
fn label(request: &Request, waiting: Waiting) -> &'static str {
    if waiting == Waiting::Automatic {
        "Refreshing night plans"
    } else if request.override_draft.is_some() || request.ts_draft.is_some() {
        "Planning an edited night"
    } else if request.rigs.as_ref().is_some_and(|rigs| rigs.len() == 1) {
        "Planning a rig's nights"
    } else {
        "Planning nights"
    }
}

/// When a view computed at `at` ages, with nothing it read changed: soon,
/// when it plans the night under way; when its first night begins; when a
/// forecast it used expires.
fn ages_at(view: &View, at: u64) -> Option<u64> {
    let nights = || view.rigs.iter().flat_map(|rig| &rig.nights);
    let under_way = nights().any(|night| night.noon_ms <= at && at < night.noon_ms + DAY_MS);
    let begins = nights()
        .map(|night| night.noon_ms)
        .filter(|noon| *noon > at)
        .min();
    let expires = view
        .rigs
        .iter()
        .filter_map(|rig| rig.forecast.as_ref()?.expires_at_ms())
        .filter(|expires| *expires > at)
        .min();
    [under_way.then_some(at + UNDER_WAY_MS), begins, expires]
        .into_iter()
        .flatten()
        .min()
}

/// Whether a view of the nights from the one under way has fallen a night
/// behind: its first night has ended.
fn behind(view: &View, now: u64) -> bool {
    view.rigs.iter().any(|rig| {
        rig.nights
            .first()
            .is_some_and(|first| now >= first.noon_ms + DAY_MS)
    })
}

impl Cache {
    /// The night plans `request` asks for: kept, or computed now.
    /// `refresh` computes them again whatever is kept.
    pub(super) async fn plan(
        &self,
        state: &Arc<AppState>,
        service: &Arc<Service>,
        request: Request,
        refresh: bool,
    ) -> Outcome {
        let Some(key) = key(&request) else {
            return self.run_once(state, request).await;
        };
        let reading = self.readings.fetch_add(1, Ordering::AcqRel);
        if !refresh && let Some(kept) = self.kept(state, service, &key, &request, reading)? {
            return Ok(kept);
        }
        self.wait(state, service, key, request, reading, Waiting::Person)
            .await
    }

    /// The kept view, when it can answer now; it starts its computation
    /// again when it aged.
    fn kept(
        &self,
        state: &Arc<AppState>,
        service: &Arc<Service>,
        key: &str,
        request: &Request,
        reading: u64,
    ) -> Result<Option<Kept>, NightPlanError> {
        let now = now_ms();
        let inputs = Inputs::read(state, service);
        let mut entries = self.entries.lock().map_err(|_| Error::Internal)?;
        let Some(entry) = entries.get_mut(key) else {
            return Ok(None);
        };
        entry.read_at_ms = now;
        if entry.inputs != inputs.of(&entry.catalogs) {
            return Ok(None);
        }
        let aged = entry.ages_at_ms.is_some_and(|at| now >= at);
        if aged
            && (now.saturating_sub(entry.kept.computed_at_ms) > STALE_MS
                || (request.first_night.is_none() && behind(&entry.kept.view, now)))
        {
            return Ok(None);
        }
        let paused = now < entry.paused_until_ms;
        let kept = entry.kept.clone();
        drop(entries);
        let running = self.running(key);
        if aged && !paused && !running {
            self.start(
                state,
                service,
                key.to_owned(),
                request.clone(),
                reading,
                Waiting::Automatic,
            );
        }
        Ok(Some(Kept {
            refreshing: running || (aged && !paused),
            ..kept
        }))
    }

    /// Compute again, one at a time and as no one's work, each view read in
    /// the last [`VIEWED_MS`] whose inputs changed or that aged, and let go
    /// of views no one read for [`KEPT_MS`].
    pub(super) async fn refresh(&self, state: &Arc<AppState>, service: &Arc<Service>) {
        let now = now_ms();
        let inputs = Inputs::read(state, service);
        let due: Vec<(String, Request)> = {
            let Ok(mut entries) = self.entries.lock() else {
                return;
            };
            entries.retain(|_, entry| now.saturating_sub(entry.read_at_ms) < KEPT_MS);
            entries
                .iter()
                .filter(|(_, entry)| {
                    now.saturating_sub(entry.read_at_ms) < VIEWED_MS
                        && now >= entry.paused_until_ms
                        && (entry.inputs != inputs.of(&entry.catalogs)
                            || entry.ages_at_ms.is_some_and(|at| now >= at))
                })
                .map(|(key, entry)| (key.clone(), entry.request.clone()))
                .collect()
        };
        for (key, request) in due {
            if state.shutdown.is_stopping() {
                return;
            }
            let reading = self.readings.fetch_add(1, Ordering::AcqRel);
            if let Err(error) = self
                .wait(state, service, key, request, reading, Waiting::Automatic)
                .await
            {
                tracing::debug!(error = ?error, "night plans could not be computed again");
            }
        }
    }

    fn running(&self, key: &str) -> bool {
        self.flights.lock().is_ok_and(|flights| {
            flights
                .get(key)
                .is_some_and(|flight| flight.receiver.borrow().is_none())
        })
    }

    /// Wait for a computation of `key` that read the inputs after
    /// `reading`: join the one under way when it did, or wait for it to end
    /// and start another.
    async fn wait(
        &self,
        state: &Arc<AppState>,
        service: &Arc<Service>,
        key: String,
        request: Request,
        reading: u64,
        waiting: Waiting,
    ) -> Outcome {
        loop {
            let mut receiver = self.start(
                state,
                service,
                key.clone(),
                request.clone(),
                reading,
                waiting,
            );
            let outcome = loop {
                if let Some(outcome) = receiver.borrow_and_update().clone() {
                    break outcome;
                }
                if receiver.changed().await.is_err() {
                    break Err(Error::Internal.into());
                }
            };
            match outcome {
                Ok(kept) if kept.reading > reading => return Ok(kept),
                // It read the inputs before this request did.
                Ok(_) => continue,
                // Stopped from the header: everyone waiting gets the view
                // kept, if there is one.
                Err(NightPlanError::Stopped) => {
                    let kept = self
                        .entries
                        .lock()
                        .ok()
                        .and_then(|entries| entries.get(&key).map(|entry| entry.kept.clone()));
                    return kept.ok_or(NightPlanError::Stopped);
                }
                Err(error) => return Err(error),
            }
        }
    }

    /// The computation of `key` under way, or a new one. It runs on its own
    /// task, so the view is kept even when the request that asked for it
    /// goes away.
    fn start(
        &self,
        state: &Arc<AppState>,
        service: &Arc<Service>,
        key: String,
        request: Request,
        reading: u64,
        waiting: Waiting,
    ) -> watch::Receiver<Option<Outcome>> {
        let Ok(mut flights) = self.flights.lock() else {
            let (_, receiver) = watch::channel(Some(Err(Error::Internal.into())));
            return receiver;
        };
        if let Some(flight) = flights.get(&key)
            && flight.receiver.borrow().is_none()
        {
            return flight.receiver.clone();
        }
        // After every reading made so far, this request's among them.
        let started = self
            .readings
            .fetch_add(1, Ordering::AcqRel)
            .max(reading + 1);
        let (sender, receiver) = watch::channel(None);
        flights.insert(
            key.clone(),
            Flight {
                receiver: receiver.clone(),
                reading: started,
            },
        );
        drop(flights);
        let state = Arc::clone(state);
        let service = Arc::clone(service);
        tokio::spawn(async move {
            let cache = &service.night_plans;
            let outcome = cache
                .compute_kept(&state, &service, &key, request, started, waiting)
                .await;
            // Answer before the flight goes, so a request that joined it gets
            // this outcome, and one after it finds the kept view.
            let _ = sender.send(Some(outcome));
            if let Ok(mut flights) = cache.flights.lock()
                && flights
                    .get(&key)
                    .is_some_and(|flight| flight.reading == started)
            {
                flights.remove(&key);
            }
        });
        receiver
    }

    async fn compute_kept(
        &self,
        state: &Arc<AppState>,
        service: &Arc<Service>,
        key: &str,
        request: Request,
        reading: u64,
        waiting: Waiting,
    ) -> Outcome {
        let previews = &state.previews.night_plans;
        let run = match waiting {
            Waiting::Person => previews.start(state, key.to_owned(), label(&request, waiting)),
            Waiting::Automatic => {
                previews.start_unattended(key.to_owned(), label(&request, waiting))
            }
        };
        // Read before computing: a change made while it runs shows as
        // changed on the next read.
        let inputs = Inputs::read(state, service);
        let computed_at_ms = now_ms();
        let computed = compute(state, request.clone(), run.stop()).await;
        #[cfg(test)]
        {
            let hold = self.hold.lock().unwrap().clone();
            if let Some(hold) = hold {
                hold.acquire().await.unwrap().forget();
            }
        }
        // A Stop that came as it finished counts too.
        let computed = match computed {
            Ok(_) if run.stop().requested() => Err(NightPlanError::Stopped),
            other => other,
        };
        let view = match computed {
            Ok(view) => view,
            Err(NightPlanError::Stopped) => {
                if let Ok(mut entries) = self.entries.lock()
                    && let Some(entry) = entries.get_mut(key)
                {
                    entry.paused_until_ms = now_ms() + UNDER_WAY_MS;
                }
                return Err(NightPlanError::Stopped);
            }
            Err(error) => return Err(error),
        };
        let catalogs = view.catalogs.clone();
        let ages_at_ms = ages_at(&view, computed_at_ms);
        let kept = Kept {
            view: Arc::new(view),
            computed_at_ms,
            refreshing: false,
            reading,
        };
        let mut entries = self.entries.lock().map_err(|_| Error::Internal)?;
        let read_at_ms = entries
            .get(key)
            .map_or(computed_at_ms, |entry| entry.read_at_ms);
        entries.insert(
            key.to_owned(),
            Entry {
                request,
                kept: kept.clone(),
                inputs: inputs.of(&catalogs),
                catalogs,
                ages_at_ms,
                read_at_ms,
                paused_until_ms: 0,
            },
        );
        while entries.len() > MAX_KEPT {
            let Some(oldest) = entries
                .iter()
                .filter(|(other, _)| other.as_str() != key)
                .min_by_key(|(_, entry)| entry.read_at_ms)
                .map(|(other, _)| other.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
        Ok(kept)
    }

    /// A request that is not kept: computed now as a person's work. A
    /// newer request for the same edited night stops it.
    async fn run_once(&self, state: &Arc<AppState>, request: Request) -> Outcome {
        let key = match (&request.override_draft, &request.ts_draft) {
            (Some(draft), _) => format!("edit:{}:{}", draft.rig, draft.date),
            (None, Some(draft)) => format!("edit:{}", draft.rig),
            (None, None) => format!("request:{}", self.next.fetch_add(1, Ordering::Relaxed)),
        };
        let run = state
            .previews
            .night_plans
            .start(state, key, label(&request, Waiting::Person));
        let computed_at_ms = now_ms();
        let view = compute(state, request, run.stop()).await?;
        if !run.is_latest() {
            return Err(NightPlanError::Stopped);
        }
        Ok(Kept {
            view: Arc::new(view),
            computed_at_ms,
            refreshing: false,
            reading: 0,
        })
    }
}

#[cfg(test)]
impl Cache {
    /// Age every kept view, as the night under way does after a while.
    pub(in crate::server::director) fn age_for_test(&self) {
        for entry in self.entries.lock().unwrap().values_mut() {
            entry.ages_at_ms = Some(0);
        }
    }

    /// Make every kept view `ms` older.
    pub(in crate::server::director) fn backdate_for_test(&self, ms: u64) {
        for entry in self.entries.lock().unwrap().values_mut() {
            entry.kept.computed_at_ms = entry.kept.computed_at_ms.saturating_sub(ms);
        }
    }

    /// How many views are kept.
    pub(in crate::server::director) fn kept_for_test(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// Hold every computation from now on, once computed and before it is
    /// kept, until the semaphore gets a permit for it.
    pub(in crate::server::director) fn hold_for_test(&self) -> Arc<tokio::sync::Semaphore> {
        let hold = Arc::new(tokio::sync::Semaphore::new(0));
        *self.hold.lock().unwrap() = Some(Arc::clone(&hold));
        hold
    }

    /// Whether a computation is under way.
    pub(in crate::server::director) fn computing_for_test(&self) -> bool {
        self.flights
            .lock()
            .unwrap()
            .values()
            .any(|flight| flight.receiver.borrow().is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(body: serde_json::Value) -> Request {
        serde_json::from_value(body).unwrap()
    }

    #[test]
    fn a_request_is_kept_by_what_it_asks_for_and_never_with_edits_or_fixed_instants() {
        let (a, b) = (Uuid::from_u128(1), Uuid::from_u128(2));
        assert_eq!(
            key(&request(serde_json::json!({"rigs": [a, b], "nights": 3}))),
            key(&request(
                serde_json::json!({"rigs": [b, a], "nights": 3, "refresh": true})
            )),
        );
        assert_ne!(
            key(&request(serde_json::json!({"nights": 3}))),
            key(&request(serde_json::json!({"nights": 4}))),
        );
        for fixed in [
            serde_json::json!({"start_ms": 1_790_294_400_000u64}),
            serde_json::json!({"plan_from_ms": 1_790_294_400_000u64}),
            serde_json::json!({"ts_draft": {"rig": a, "changes": {}}}),
            serde_json::json!({"override_draft": {"rig": a, "date": "2026-09-24", "entries": []}}),
        ] {
            assert_eq!(key(&request(fixed.clone())), None, "{fixed}");
        }
    }
}
