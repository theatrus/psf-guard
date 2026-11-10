//! The plugin's check-in and live status. Check-in stores one page of
//! journaled receipts from one ledger and acknowledges the contiguous cursor;
//! status keeps the newest coalesced report per rig. Neither changes a plan,
//! a rig database or the program; both tell the plugin whether to pull again.

use super::*;
use crate::server::database_context::DatabaseContext;
use psf_guard_director_meta::inbox::{Contact, ContactKind, Receipt, RigStatus, Stored, MAX_PAGE};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// How long after its last call a rig still counts as connected, and after
/// how long it is written off rather than merely quiet.
const ONLINE_MS: u64 = 3 * 60 * 1000;
const STALE_MS: u64 = 30 * 60 * 1000;
/// A live status older than this is shown as stale: the rig may have moved
/// on, so "exposing" ten minutes ago must not read as exposing now.
const STATUS_FRESH_MS: u64 = 10 * 60 * 1000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CheckIn {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    ledger_id: String,
    /// The `revision` of the program the plugin is running, if it has one.
    #[serde(default)]
    program_revision: Option<String>,
    /// Ledger `ExecutionEvent`s as the sidecar emits them, ascending, one
    /// ledger per page. Kept verbatim; only identity fields are read here.
    events: Vec<serde_json::Value>,
}

#[derive(Serialize)]
pub(super) struct Acknowledgement {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    rig_id: Uuid,
    ledger_id: String,
    /// Every sequence up to and including this one is stored here.
    acknowledged_through: u64,
    highest_seen: u64,
    outcomes: Vec<Stored>,
    applied: usize,
    duplicates: usize,
    /// Sequences whose content differed from what is already stored. They
    /// were not applied and are not acknowledged.
    conflicts: Vec<u64>,
    program_revision: Option<String>,
    program_changed: bool,
    received_at_ms: u64,
}

pub(super) async fn check_in(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Json(request): Json<CheckIn>,
) -> Result<Json<ApiResponse<Acknowledgement>>, program::PullError> {
    check_in_feed(state, rig, request, false).await
}

pub(super) async fn operation_check_in(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Json(request): Json<CheckIn>,
) -> Result<Json<ApiResponse<Acknowledgement>>, program::PullError> {
    check_in_feed(state, rig, request, true).await
}

async fn check_in_feed(
    state: Arc<AppState>,
    rig: Uuid,
    request: CheckIn,
    operations: bool,
) -> Result<Json<ApiResponse<Acknowledgement>>, program::PullError> {
    let service = enabled(&state)?;
    if request.coordinator_instance_id != service.instance_id {
        return Err(Error::WrongRig.into());
    }
    let limit = if operations {
        psf_guard_director_meta::operation_inbox::MAX_PAGE
    } else {
        MAX_PAGE
    };
    if request.events.is_empty() || request.events.len() > limit {
        return Err(Error::Invalid.into());
    }
    let receipts = if operations {
        Vec::new()
    } else {
        request
            .events
            .iter()
            .map(|event| receipt_from(event, rig, &request.ledger_id))
            .collect::<Result<Vec<_>, Error>>()?
    };
    let operation_events = if operations {
        request
            .events
            .iter()
            .map(|value| {
                let event: psf_guard_director_meta::operation_inbox::Event =
                    serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
                if event.ledger_id != request.ledger_id || event.rig_id != rig.to_string() {
                    return Err(Error::Invalid);
                }
                Ok(event)
            })
            .collect::<Result<Vec<_>, Error>>()?
    } else {
        Vec::new()
    };
    let sequences: Vec<_> = request
        .events
        .iter()
        .map(|event| event["sequence"].as_u64().ok_or(Error::Invalid))
        .collect::<Result<_, _>>()?;
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let ack = service
        .clone()
        .with_writer(move |store| {
            store
                .catalog_rig(request.catalog_id)?
                .filter(|binding| binding.rig.id == rig)
                .ok_or(Error::WrongRig)?;
            let now = now_ms();
            let (outcomes, cursor) = if operations {
                store.store_operation_receipts(rig, &operation_events, now)?
            } else {
                store.store_receipts(&receipts, now)?
            };
            store.record_contact(rig, ContactKind::CheckIn, now, Some(&request.ledger_id))?;
            let program_revision = program::current_revision(
                store,
                &catalogs,
                service.instance_id,
                rig,
                request.catalog_id,
            )?;
            let program_changed = match (&program_revision, &request.program_revision) {
                (Some(current), Some(held)) => current != held,
                (Some(_), None) => true,
                (None, _) => false,
            };
            Ok::<_, program::PullError>(Acknowledgement {
                coordinator_instance_id: service.instance_id,
                catalog_id: request.catalog_id,
                rig_id: rig,
                ledger_id: request.ledger_id.clone(),
                acknowledged_through: cursor.highest_contiguous,
                highest_seen: cursor.highest_seen,
                applied: outcomes.iter().filter(|o| **o == Stored::Applied).count(),
                duplicates: outcomes.iter().filter(|o| **o == Stored::Duplicate).count(),
                conflicts: outcomes
                    .iter()
                    .zip(&sequences)
                    .filter(|(o, _)| **o == Stored::Conflict)
                    .map(|(_, sequence)| *sequence)
                    .collect(),
                outcomes,
                program_revision,
                program_changed,
                received_at_ms: now,
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(ack)))
}

/// Pull the identity fields out of a ledger event without pinning its schema.
fn receipt_from(event: &serde_json::Value, rig: Uuid, ledger: &str) -> Result<Receipt, Error> {
    let text = |value: &serde_json::Value| value.as_str().map(str::to_owned);
    let event_ledger = text(&event["ledger_id"]).ok_or(Error::Invalid)?;
    let event_rig = text(&event["rig_id"]).ok_or(Error::Invalid)?;
    if event_ledger != ledger || event_rig != rig.to_string() {
        return Err(Error::Invalid);
    }
    let sequence = event["sequence"].as_u64().ok_or(Error::Invalid)?;
    let attempt = &event["attempt"];
    let state = text(&attempt["evidence"]["state"]).ok_or(Error::Invalid)?;
    Ok(Receipt {
        rig_id: rig,
        ledger_id: event_ledger,
        sequence,
        goal_id: text(&attempt["goal_id"]).ok_or(Error::Invalid)?,
        capture_id: text(&attempt["capture_id"]).ok_or(Error::Invalid)?,
        state,
        payload: event.clone(),
        received_at_ms: 0,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusReport {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    /// Changes when the plugin starts a new acquisition session; a late report
    /// from an old session never overwrites the current one.
    session_id: String,
    reported_at_ms: u64,
    #[serde(default)]
    program_revision: Option<String>,
    /// Coalesced live status as the plugin describes it: phase, goal and
    /// target IDs, elapsed time, wait reason, safety, connectivity and queue
    /// depth. Stored verbatim for the operator view.
    status: serde_json::Value,
}

#[derive(Serialize)]
pub(super) struct StatusAnswer {
    accepted: bool,
    program_revision: Option<String>,
    program_changed: bool,
    received_at_ms: u64,
}

pub(super) async fn report_status(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Json(request): Json<StatusReport>,
) -> Result<Json<ApiResponse<StatusAnswer>>, program::PullError> {
    let service = enabled(&state)?;
    if request.coordinator_instance_id != service.instance_id {
        return Err(Error::WrongRig.into());
    }
    if !request.status.is_object() {
        return Err(Error::Invalid.into());
    }
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let answer = service
        .clone()
        .with_writer(move |store| {
            store
                .catalog_rig(request.catalog_id)?
                .filter(|binding| binding.rig.id == rig)
                .ok_or(Error::WrongRig)?;
            let now = now_ms();
            let status = RigStatus {
                rig_id: rig,
                session_id: request.session_id.clone(),
                reported_at_ms: request.reported_at_ms,
                payload: request.status.clone(),
                received_at_ms: now,
            };
            let accepted = store.record_status(&status)?;
            store.record_contact(rig, ContactKind::Status, now, Some(&request.session_id))?;
            let program_revision = program::current_revision(
                store,
                &catalogs,
                service.instance_id,
                rig,
                request.catalog_id,
            )?;
            let program_changed = match (&program_revision, &request.program_revision) {
                (Some(current), Some(held)) => current != held,
                (Some(_), None) => true,
                (None, _) => false,
            };
            Ok::<_, program::PullError>(StatusAnswer {
                accepted,
                program_revision,
                program_changed,
                received_at_ms: now,
            })
        })
        .await?;
    if answer.accepted {
        collaboration_auth::forward_status(state, rig);
    }
    Ok(Json(ApiResponse::success(answer)))
}

#[derive(Serialize)]
struct ContactView {
    at_ms: u64,
    detail: Option<String>,
}

#[derive(Default, Serialize)]
struct Contacts {
    program_pull: Option<ContactView>,
    check_in: Option<ContactView>,
    status: Option<ContactView>,
}

/// Derived from server receipt times only. `never` means no call yet;
/// `offline` means nothing for half an hour. A rig is never shown as doing
/// anything on the strength of an old report.
#[derive(Serialize)]
struct Connectivity {
    state: &'static str,
    last_contact_ms: Option<u64>,
    age_ms: Option<u64>,
}

#[derive(Clone, Serialize)]
struct AssignmentView {
    project: NamedIdentity,
    activation_revision: u64,
    applied_at_ms: u64,
}

#[derive(Serialize)]
pub(super) struct RigStatusView {
    weather: psf_guard_director_meta::weather::RigWeather,
    rig: NamedIdentity,
    catalog_slug: Option<String>,
    catalog_name: Option<String>,
    /// The newest coalesced report, or `None` before the first.
    status: Option<RigStatus>,
    status_age_ms: Option<u64>,
    /// The report exceeded its freshness lease; show it as history, not now.
    status_stale: bool,
    /// The last acknowledged cursor per ledger this rig has checked in with.
    checkins: Vec<psf_guard_director_meta::inbox::FeedCursor>,
    contacts: Contacts,
    connectivity: Connectivity,
    /// Activated plans this rig is part of.
    assignments: Vec<AssignmentView>,
    /// Saved captures the rig has reported that grading has not yet turned
    /// into accepted frames.
    pending_receipts: u32,
    /// Historical receipts, never a substitute for the coalesced live report.
    recent_operations: Vec<psf_guard_director_meta::operation_inbox::OperationReceipt>,
    /// The Sync plugin on this rig's database: its last contact and action,
    /// and what N.I.N.A.'s sequence is doing. `None` without Sync.
    sync: Option<crate::server::sync_sequence::SyncLive>,
    /// Projects in the rig's database that Target Scheduler has Active,
    /// including those a Director activation wrote. `None` when the
    /// database could not be read.
    active_projects: Option<u32>,
}

/// How long a database's count of Active projects is kept. The count
/// changes only when someone changes a project, and reading it opens the
/// rig's database, often on a network share.
const ACTIVE_PROJECTS_KEPT_MS: u64 = 60_000;

/// Each database's count of Active projects, by file path, with when it was
/// read. N.I.N.A. changes project states too, so unlike the rigs' state
/// this is read again after a while rather than updated as PSF Guard writes.
type ActiveCounts = HashMap<String, (u64, Option<u32>)>;
static ACTIVE_PROJECTS: std::sync::LazyLock<Mutex<ActiveCounts>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Each database's count of Active projects, by its ID and file path, read
/// off the request path and kept for a minute. A file that cannot be read
/// has no count.
async fn active_projects(paths: Vec<(String, String)>) -> HashMap<String, u32> {
    let now = now_ms();
    let kept = |path: &str| {
        ACTIVE_PROJECTS.lock().ok().and_then(|counts| {
            counts
                .get(path)
                .filter(|(at, _)| now.saturating_sub(*at) < ACTIVE_PROJECTS_KEPT_MS)
                .map(|(_, count)| *count)
        })
    };
    let mut counts = HashMap::new();
    let mut stale = Vec::new();
    for (id, path) in paths {
        match kept(&path) {
            Some(Some(count)) => {
                counts.insert(id, count);
            }
            Some(None) => {}
            None => stale.push((id, path)),
        }
    }
    if stale.is_empty() {
        return counts;
    }
    let read = tokio::task::spawn_blocking(move || {
        stale
            .into_iter()
            .map(|(id, path)| {
                let count = count_active(&path);
                (id, path, count)
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    if let Ok(mut kept) = ACTIVE_PROJECTS.lock() {
        for (_, path, count) in &read {
            kept.insert(path.clone(), (now, *count));
        }
    }
    counts.extend(
        read.into_iter()
            .filter_map(|(id, _, count)| Some((id, count?))),
    );
    counts
}

fn count_active(path: &str) -> Option<u32> {
    let connection = crate::server::database_context::open_scheduler_connection_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .ok()?;
    // Live polls every few seconds: a database another writer holds is left
    // without a count rather than waited on.
    connection
        .busy_timeout(std::time::Duration::from_millis(250))
        .ok()?;
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM project WHERE state=1", [], |row| {
            row.get(0)
        })
        .ok()?;
    Some(u32::try_from(count).unwrap_or(u32::MAX))
}

fn contacts(list: Vec<Contact>) -> Contacts {
    let mut contacts = Contacts::default();
    for contact in list {
        let view = ContactView {
            at_ms: contact.at_ms,
            detail: contact.detail,
        };
        match contact.kind {
            ContactKind::ProgramPull => contacts.program_pull = Some(view),
            ContactKind::CheckIn => contacts.check_in = Some(view),
            ContactKind::Status => contacts.status = Some(view),
        }
    }
    contacts
}

/// `sync_contact_ms`: the Sync plugin's newest call, which counts as contact
/// too.
fn connectivity(contacts: &Contacts, sync_contact_ms: Option<u64>, now: u64) -> Connectivity {
    let last = [&contacts.program_pull, &contacts.check_in, &contacts.status]
        .into_iter()
        .flatten()
        .map(|c| c.at_ms)
        .chain(sync_contact_ms)
        .max();
    let age = last.map(|at| now.saturating_sub(at));
    Connectivity {
        state: match age {
            None => "never",
            Some(age) if age <= ONLINE_MS => "online",
            Some(age) if age <= STALE_MS => "stale",
            Some(_) => "offline",
        },
        last_contact_ms: last,
        age_ms: age,
    }
}

/// The registered databases, as one list.
fn registered(state: &AppState) -> Result<Vec<Arc<DatabaseContext>>, Error> {
    Ok(state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect())
}

/// What every rig's Sync plugin last reported, and when each database last
/// received a Director upload. Both are kept in memory.
fn plugin_contacts(
    state: &AppState,
    catalogs: &[Arc<DatabaseContext>],
    now: u64,
) -> (
    HashMap<String, crate::server::sync_sequence::SyncLive>,
    HashMap<String, u64>,
) {
    let sync = catalogs
        .iter()
        .filter_map(|catalog| {
            crate::server::sync_sequence::SyncLive::for_catalog(state, catalog, now)
                .map(|view| (catalog.id.clone(), view))
        })
        .collect();
    let uploads = catalogs
        .iter()
        .filter_map(|catalog| {
            let at = state.remote_audit.director_upload_at(&catalog.id)?;
            Some((catalog.id.clone(), at.max(0) as u64))
        })
        .collect();
    (sync, uploads)
}

/// The report exceeded its freshness lease; it is history, not now.
fn status_is_stale(status: Option<&RigStatus>, now: u64) -> bool {
    status.is_some_and(|status| {
        now.saturating_sub(status.received_at_ms)
            > status.payload["fresh_for_ms"]
                .as_u64()
                .unwrap_or(STATUS_FRESH_MS)
                .clamp(15_000, STATUS_FRESH_MS)
    })
}

/// The Director plugin's phases that count as exposing.
const EXPOSING_PHASES: [&str; 3] = ["exposing", "imaging", "capturing"];

/// The plugin whose report says what a rig is doing now, and whether that
/// rig is exposing. A rig may run the Director plugin, the Sync plugin or
/// both. The Director plugin's report speaks while it is fresh; otherwise a
/// fresh Sync sequence report does; with neither fresh, the Director report
/// stands, then the Sync one. A rig is exposing when a fresh report from
/// either plugin says so.
fn rig_now(
    status: Option<&RigStatus>,
    status_stale: bool,
    sync: Option<&crate::server::sync_sequence::SyncLive>,
) -> (Option<&'static str>, bool) {
    let sequence = sync.and_then(|sync| sync.sequence.as_ref().map(|s| (s, sync.sequence_stale)));
    let report = match (status, sequence) {
        (Some(_), Some((_, false))) if status_stale => Some("sync"),
        (Some(_), _) => Some("director"),
        (None, Some(_)) => Some("sync"),
        (None, None) => None,
    };
    let director_exposing = status.is_some_and(|status| {
        let payload = &status.payload;
        let phase = payload["phase"]
            .as_str()
            .or_else(|| payload["state"].as_str())
            .unwrap_or("");
        !status_stale && EXPOSING_PHASES.contains(&phase.trim().to_lowercase().as_str())
    });
    let sync_exposing = sequence.is_some_and(|(sequence, stale)| {
        !stale
            && sequence.running
            && sequence
                .activity
                .as_ref()
                .is_some_and(|activity| activity.kind == "exposing")
    });
    (report, director_exposing || sync_exposing)
}

/// A rig's link state, counting a Sync call or a Director upload as contact.
fn rig_connectivity(
    entry: &super::live_rigs::LiveRig,
    sync: Option<&crate::server::sync_sequence::SyncLive>,
    uploads: &HashMap<String, u64>,
    now: u64,
) -> (Contacts, Connectivity) {
    let contacts = contacts(entry.contacts.clone());
    let director_upload = entry
        .catalog_slug
        .as_ref()
        .and_then(|slug| uploads.get(slug).copied());
    let connectivity = connectivity(
        &contacts,
        sync.and_then(|sync| sync.last_contact_ms)
            .max(director_upload),
        now,
    );
    (contacts, connectivity)
}

/// What the Live table reads from the store for one rig, beside its status
/// and contacts.
#[derive(Clone)]
struct RigExtras {
    weather: psf_guard_director_meta::weather::RigWeather,
    checkins: Vec<psf_guard_director_meta::inbox::FeedCursor>,
    assignments: Vec<AssignmentView>,
    pending_receipts: u32,
    recent_operations: Vec<psf_guard_director_meta::operation_inbox::OperationReceipt>,
}

/// Weather is judged against the clock (a report's freshness, the forecast
/// hour), so it is judged again at least this often.
const WEATHER_JUDGED_FOR: std::time::Duration = std::time::Duration::from_secs(60);

struct TableSnapshot {
    rigs: Vec<Uuid>,
    planning_rows: u64,
    weather_reports: u64,
    judged_at: std::time::Instant,
    extras: Arc<HashMap<Uuid, RigExtras>>,
}

/// The Live table's reads, kept until a store write could change them: any
/// planning write, or a weather report. Status and contacts come from
/// [`super::live_rigs`].
#[derive(Default)]
pub(super) struct LiveTable {
    kept: Mutex<Option<TableSnapshot>>,
}

impl Service {
    async fn live_table(
        self: Arc<Self>,
        ids: Vec<Uuid>,
    ) -> Result<Arc<HashMap<Uuid, RigExtras>>, Error> {
        use std::sync::atomic::Ordering;
        // Read before the store, so a write that lands meanwhile leaves the
        // count behind and the next poll reads again.
        let planning_rows = self.planning_rows.load(Ordering::Acquire);
        let weather_reports = self.liveness.weather_reports();
        if let Ok(kept) = self.live_table.kept.lock()
            && let Some(snapshot) = kept.as_ref().filter(|snapshot| {
                snapshot.rigs == ids
                    && snapshot.planning_rows == planning_rows
                    && snapshot.weather_reports == weather_reports
                    && snapshot.judged_at.elapsed() < WEATHER_JUDGED_FOR
            })
        {
            return Ok(snapshot.extras.clone());
        }
        let read = ids.clone();
        let extras = self
            .clone()
            .query(move |store| {
                let now = now_ms();
                let mut extras = HashMap::new();
                for rig_id in read {
                    let mut assignments = Vec::new();
                    for activation in store.activations_for_rig(rig_id)? {
                        if let Some(project) = store.project(activation.project_id)? {
                            assignments.push(AssignmentView {
                                project,
                                activation_revision: activation.revision,
                                applied_at_ms: activation.applied_at_ms,
                            });
                        }
                    }
                    assignments.sort_by(|a, b| a.project.name.cmp(&b.project.name));
                    let pending_receipts = store
                        .saved_captures_by_goal(rig_id)?
                        .into_iter()
                        .map(|(_, count)| count)
                        .fold(0u32, u32::saturating_add);
                    extras.insert(
                        rig_id,
                        RigExtras {
                            weather: store.rig_weather(rig_id, None, now)?,
                            checkins: store.feed_cursors_for_rig(rig_id)?,
                            assignments,
                            pending_receipts,
                            recent_operations: store.recent_operations(rig_id)?,
                        },
                    );
                }
                Ok(extras)
            })
            .await?;
        let extras = Arc::new(extras);
        if let Ok(mut kept) = self.live_table.kept.lock() {
            *kept = Some(TableSnapshot {
                rigs: ids,
                planning_rows,
                weather_reports,
                judged_at: std::time::Instant::now(),
                extras: extras.clone(),
            });
        }
        Ok(extras)
    }
}

/// Operator view: every bound rig with its connectivity, newest status,
/// check-in cursors, assignments and pending receipts. Rigs that have
/// reported but lost their database binding are listed too.
pub(super) async fn statuses(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<RigStatusView>>>, Error> {
    let catalogs = registered(&state)?;
    let service = enabled(&state)?;
    let mut active = active_projects(
        catalogs
            .iter()
            .map(|catalog| (catalog.id.clone(), catalog.database_path.clone()))
            .collect(),
    )
    .await;
    let now = now_ms();
    let (mut sync, uploads) = plugin_contacts(&state, &catalogs, now);
    let rigs = service.clone().live_rigs(catalogs).await?;
    let extras = service
        .live_table(rigs.iter().map(|entry| entry.rig.id).collect())
        .await?;
    let mut views = Vec::new();
    for entry in rigs.iter() {
        // A rig the kept reads lack has just appeared; the next poll has it.
        let Some(extra) = extras.get(&entry.rig.id).cloned() else {
            continue;
        };
        let sync = entry
            .catalog_slug
            .as_ref()
            .and_then(|slug| sync.remove(slug));
        let active_projects = entry
            .catalog_slug
            .as_ref()
            .and_then(|slug| active.remove(slug));
        let (contacts, connectivity) = rig_connectivity(entry, sync.as_ref(), &uploads, now);
        let status = entry.status.clone();
        views.push(RigStatusView {
            weather: extra.weather,
            rig: entry.rig.clone(),
            catalog_slug: entry.catalog_slug.clone(),
            catalog_name: entry.catalog_name.clone(),
            status_stale: status_is_stale(status.as_ref(), now),
            status_age_ms: status
                .as_ref()
                .map(|s| now.saturating_sub(s.received_at_ms)),
            status,
            checkins: extra.checkins,
            contacts,
            connectivity,
            assignments: extra.assignments,
            pending_receipts: extra.pending_receipts,
            recent_operations: extra.recent_operations,
            sync,
            active_projects,
        });
    }
    Ok(Json(ApiResponse::success(views)))
}

/// What each rig is doing now, for the header's Rigs tab, the Sky and Night
/// details. Read from memory alone: no database is opened.
#[derive(Serialize)]
pub(super) struct RigSummaryView {
    rig: NamedIdentity,
    catalog_slug: Option<String>,
    catalog_name: Option<String>,
    status: Option<RigStatus>,
    status_age_ms: Option<u64>,
    status_stale: bool,
    connectivity: Connectivity,
    sync: Option<crate::server::sync_sequence::SyncLive>,
    /// `director` or `sync`: the plugin whose report says what the rig is
    /// doing now; `None` before either reports.
    report: Option<&'static str>,
    /// A fresh report from either plugin says the rig is exposing.
    exposing: bool,
}

pub(super) async fn summary(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<RigSummaryView>>>, Error> {
    let catalogs = registered(&state)?;
    let service = enabled(&state)?;
    let now = now_ms();
    let (mut sync, uploads) = plugin_contacts(&state, &catalogs, now);
    let rigs = service.live_rigs(catalogs).await?;
    let views = rigs
        .iter()
        .map(|entry| {
            let sync = entry
                .catalog_slug
                .as_ref()
                .and_then(|slug| sync.remove(slug));
            let (_, connectivity) = rig_connectivity(entry, sync.as_ref(), &uploads, now);
            let status_stale = status_is_stale(entry.status.as_ref(), now);
            let (report, exposing) = rig_now(entry.status.as_ref(), status_stale, sync.as_ref());
            RigSummaryView {
                rig: entry.rig.clone(),
                catalog_slug: entry.catalog_slug.clone(),
                catalog_name: entry.catalog_name.clone(),
                status_age_ms: entry
                    .status
                    .as_ref()
                    .map(|s| now.saturating_sub(s.received_at_ms)),
                status: entry.status.clone(),
                status_stale,
                connectivity,
                sync,
                report,
                exposing,
            }
        })
        .collect();
    Ok(Json(ApiResponse::success(views)))
}

#[cfg(test)]
mod active_project_tests {
    use super::*;

    #[tokio::test]
    async fn counts_the_projects_target_scheduler_has_active() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rig.sqlite");
        let db = crate::ts_schema::create_fresh_db(&path).unwrap();
        for (id, state) in [(1, 1), (2, 1), (3, 2), (4, 0)] {
            db.execute(
                "INSERT INTO project(Id, profileId, name, state) VALUES(?1, 'profile', ?2, ?3)",
                rusqlite::params![id, format!("Project {id}"), state],
            )
            .unwrap();
        }
        // A catalog on a local disk runs in write-ahead-log mode with the
        // server's own connection open.
        db.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))
            .unwrap();
        let missing = dir.path().join("missing.sqlite");
        let counts = active_projects(vec![
            ("rig".into(), path.to_string_lossy().into_owned()),
            ("missing".into(), missing.to_string_lossy().into_owned()),
        ])
        .await;
        assert_eq!(counts.get("rig"), Some(&2));
        assert_eq!(counts.get("missing"), None);
        drop(db);
    }
}

#[cfg(test)]
mod rig_now_tests {
    use super::*;
    use crate::server::sync_sequence::{RecentEvents, SequenceActivity, SequenceStatus, SyncLive};

    fn status(phase: &str) -> RigStatus {
        RigStatus {
            rig_id: Uuid::new_v4(),
            session_id: "s".into(),
            reported_at_ms: 1,
            payload: serde_json::json!({ "phase": phase }),
            received_at_ms: 1,
        }
    }

    fn sync(kind: &str, stale: bool) -> SyncLive {
        SyncLive {
            last_contact_ms: Some(1),
            plugin_version: None,
            latest_action: None,
            problems: Vec::new(),
            sequence: Some(SequenceStatus {
                session_id: Uuid::new_v4(),
                reported_at_ms: 1,
                received_at_ms: 1,
                fresh_for_ms: 120_000,
                running: true,
                operation: None,
                activity: Some(
                    serde_json::from_value::<SequenceActivity>(serde_json::json!({ "kind": kind }))
                        .unwrap(),
                ),
                recent: RecentEvents::default(),
            }),
            sequence_stale: stale,
        }
    }

    #[test]
    fn either_plugins_fresh_report_counts_as_exposing() {
        // Director alone.
        assert_eq!(
            rig_now(Some(&status(" Exposing ")), false, None),
            (Some("director"), true)
        );
        assert_eq!(
            rig_now(Some(&status("capturing")), true, None),
            (Some("director"), false)
        );
        assert_eq!(
            rig_now(Some(&status("slewing")), false, None),
            (Some("director"), false)
        );
        // Sync alone.
        assert_eq!(
            rig_now(None, false, Some(&sync("exposing", false))),
            (Some("sync"), true)
        );
        assert_eq!(
            rig_now(None, false, Some(&sync("exposing", true))),
            (Some("sync"), false)
        );
        assert_eq!(
            rig_now(None, false, Some(&sync("autofocus", false))),
            (Some("sync"), false)
        );
        // Both: a fresh Director report speaks; either may say exposing.
        assert_eq!(
            rig_now(
                Some(&status("Stopped")),
                false,
                Some(&sync("exposing", false))
            ),
            (Some("director"), true)
        );
        // A stale Director report gives way to a fresh Sync one.
        assert_eq!(
            rig_now(
                Some(&status("exposing")),
                true,
                Some(&sync("exposing", false))
            ),
            (Some("sync"), true)
        );
        // Neither fresh: the Director report stands.
        assert_eq!(
            rig_now(
                Some(&status("exposing")),
                true,
                Some(&sync("exposing", true))
            ),
            (Some("director"), false)
        );
        assert_eq!(rig_now(None, false, None), (None, false));
    }
}
