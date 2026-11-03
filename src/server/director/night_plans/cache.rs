//! Night plans as a person's work on the job line, and the views kept of
//! them.
//!
//! Every computation runs as a person's work (`AppState.previews`'s
//! `night_plans` runs): it holds the interactive guard, so background work
//! steps aside while it runs, and the header lists it with Stop. A newer
//! request for the same edited night stops the one before it.
//!
//! A request without unsaved edits or fixed instants is kept, by what it
//! asks for, with what it was computed from: when the meta store and each
//! rig database last changed on disk. The same request again gets the kept
//! view at once while those are unchanged. When one changed, the request
//! waits for a new computation, since someone usually just changed it. A
//! view of the night under way ages in [`UNDER_WAY_MS`], since that night
//! is planned from now: an aged view is answered at once, marked
//! `refreshing`, while a new one is computed. Requests for one view that
//! arrive together share one computation. A timer keeps the views read in
//! the last [`VIEWED_MS`] current, so a page or an agent rarely waits.

use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::watch;

/// How long a view that plans the night under way stands.
const UNDER_WAY_MS: u64 = 2 * 60_000;
/// Views read within this long are kept current by the timer.
const VIEWED_MS: u64 = 10 * 60_000;
/// Views not read for this long are let go.
const KEPT_MS: u64 = 60 * 60_000;
/// How often the timer looks for views to compute again.
pub(in crate::server::director) const REFRESH_EVERY: std::time::Duration =
    std::time::Duration::from_secs(60);

type Outcome = Result<Kept, NightPlanError>;

/// The views kept, and the computations under way.
#[derive(Default)]
pub(in crate::server::director) struct Cache {
    entries: Mutex<HashMap<String, Entry>>,
    flights: Mutex<HashMap<String, watch::Receiver<Option<Outcome>>>>,
    /// Numbers the runs of requests that are not kept, so none replaces
    /// another.
    next: AtomicU64,
}

struct Entry {
    request: Request,
    kept: Kept,
    inputs: Inputs,
    /// When the view no longer stands, though nothing it was computed from
    /// changed: the night under way is planned from now.
    ages_at_ms: Option<u64>,
    read_at_ms: u64,
}

/// A view, when it was computed, and whether it is being computed again.
#[derive(Clone)]
pub(super) struct Kept {
    pub(super) view: Arc<View>,
    pub(super) computed_at_ms: u64,
    pub(super) refreshing: bool,
}

/// What views are computed from: when each file they read last changed,
/// and whether this server may write rig databases.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Inputs {
    files: Vec<(String, Option<(SystemTime, u64)>)>,
    database_management: bool,
}

impl Inputs {
    fn read(state: &AppState, service: &Service) -> Self {
        let mut paths = vec![service.path.to_string_lossy().into_owned()];
        if let Ok(databases) = state.databases.read() {
            let mut catalogs: Vec<_> = databases
                .values()
                .map(|catalog| catalog.database_path.clone())
                .collect();
            catalogs.sort();
            paths.extend(catalogs);
        }
        let files = paths
            .into_iter()
            .flat_map(|path| [format!("{path}-wal"), path])
            .map(|path| {
                // A reader opening the database creates an empty write-ahead
                // log, which holds no change.
                let wal = path.ends_with("-wal");
                let changed = std::fs::metadata(&path)
                    .ok()
                    .filter(|meta| !(wal && meta.len() == 0))
                    .and_then(|meta| Some((meta.modified().ok()?, meta.len())));
                (path, changed)
            })
            .collect();
        Self {
            files,
            database_management: state.database_management_allowed(),
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
fn label(request: &Request) -> &'static str {
    if request.override_draft.is_some() || request.ts_draft.is_some() {
        "Planning an edited night"
    } else if request.rigs.as_ref().is_some_and(|rigs| rigs.len() == 1) {
        "Planning a rig's nights"
    } else {
        "Planning nights"
    }
}

/// When a view computed at `at` ages: soon, when it plans the night under
/// way; when its first night begins, when that is still ahead.
fn ages_at(view: &View, at: u64) -> Option<u64> {
    let nights = || view.rigs.iter().flat_map(|rig| &rig.nights);
    if nights().any(|night| night.noon_ms <= at && at < night.noon_ms + 86_400_000) {
        return Some(at + UNDER_WAY_MS);
    }
    nights()
        .map(|night| night.noon_ms)
        .filter(|noon| *noon > at)
        .min()
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
        if !refresh {
            let now = now_ms();
            let inputs = Inputs::read(state, service);
            let kept = {
                let mut entries = self.entries.lock().map_err(|_| Error::Internal)?;
                entries.get_mut(&key).and_then(|entry| {
                    entry.read_at_ms = now;
                    (entry.inputs == inputs).then(|| {
                        let aged = entry.ages_at_ms.is_some_and(|at| now >= at);
                        (entry.kept.clone(), aged)
                    })
                })
            };
            if let Some((mut kept, aged)) = kept {
                let running = self
                    .flights
                    .lock()
                    .map_err(|_| Error::Internal)?
                    .contains_key(&key);
                if aged && !running {
                    self.start(state, service, key, request);
                }
                kept.refreshing = aged || running;
                return Ok(kept);
            }
        }
        self.wait(state, service, key, request).await
    }

    /// Compute again each view read in the last [`VIEWED_MS`] whose inputs
    /// changed or that aged, one at a time, and let go of views no one read
    /// for [`KEPT_MS`].
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
                        && (entry.inputs != inputs || entry.ages_at_ms.is_some_and(|at| now >= at))
                })
                .map(|(key, entry)| (key.clone(), entry.request.clone()))
                .collect()
        };
        for (key, request) in due {
            if state.shutdown.is_stopping() {
                return;
            }
            if let Err(error) = self.wait(state, service, key, request).await {
                tracing::debug!(error = ?error, "night plans could not be computed again");
            }
        }
    }

    /// Join the computation of `key` under way, or start one, and wait for
    /// it.
    async fn wait(
        &self,
        state: &Arc<AppState>,
        service: &Arc<Service>,
        key: String,
        request: Request,
    ) -> Outcome {
        let mut receiver = self.start(state, service, key, request);
        loop {
            if let Some(outcome) = receiver.borrow_and_update().clone() {
                return outcome;
            }
            if receiver.changed().await.is_err() {
                return Err(Error::Internal.into());
            }
        }
    }

    /// The computation of `key`: the one under way, or a new one. It runs
    /// on its own task, so the view is kept even when the request that
    /// asked for it goes away.
    fn start(
        &self,
        state: &Arc<AppState>,
        service: &Arc<Service>,
        key: String,
        request: Request,
    ) -> watch::Receiver<Option<Outcome>> {
        let Ok(mut flights) = self.flights.lock() else {
            let (_, receiver) = watch::channel(Some(Err(Error::Internal.into())));
            return receiver;
        };
        if let Some(receiver) = flights.get(&key) {
            return receiver.clone();
        }
        let (sender, receiver) = watch::channel(None);
        flights.insert(key.clone(), receiver.clone());
        drop(flights);
        let state = Arc::clone(state);
        let service = Arc::clone(service);
        tokio::spawn(async move {
            let cache = &service.night_plans;
            let outcome = cache.compute_kept(&state, &service, &key, request).await;
            // Answer before the flight goes, so a request that joined it gets
            // this outcome, and one after it finds the kept view.
            let _ = sender.send(Some(outcome));
            if let Ok(mut flights) = cache.flights.lock() {
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
    ) -> Outcome {
        let run = state
            .previews
            .night_plans
            .start(state, key.to_owned(), label(&request));
        // Read before computing: a change made while it runs shows as
        // changed on the next read.
        let inputs = Inputs::read(state, service);
        let computed_at_ms = now_ms();
        let view = compute(state, request.clone(), run.stop()).await?;
        let kept = Kept::new(view, computed_at_ms);
        let ages_at_ms = ages_at(&kept.view, computed_at_ms);
        let mut entries = self.entries.lock().map_err(|_| Error::Internal)?;
        let read_at_ms = entries
            .get(key)
            .map_or(computed_at_ms, |entry| entry.read_at_ms);
        entries.insert(
            key.to_owned(),
            Entry {
                request,
                kept: kept.clone(),
                inputs,
                ages_at_ms,
                read_at_ms,
            },
        );
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
            .start(state, key, label(&request));
        let computed_at_ms = now_ms();
        let view = compute(state, request, run.stop()).await?;
        if !run.is_latest() {
            return Err(NightPlanError::Stopped);
        }
        Ok(Kept::new(view, computed_at_ms))
    }
}

impl Kept {
    fn new(view: View, computed_at_ms: u64) -> Self {
        Self {
            view: Arc::new(view),
            computed_at_ms,
            refreshing: false,
        }
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
