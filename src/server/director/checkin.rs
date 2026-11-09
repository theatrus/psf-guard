//! The plugin's check-in and live status. Check-in stores one page of
//! journaled receipts from one ledger and acknowledges the contiguous cursor;
//! status keeps the newest coalesced report per rig. Neither changes a plan,
//! a rig database or the program; both tell the plugin whether to pull again.

use super::*;
use crate::server::database_context::DatabaseContext;
use psf_guard_director_meta::inbox::{Contact, ContactKind, Receipt, RigStatus, Stored, MAX_PAGE};
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
            let accepted = store.record_status(&RigStatus {
                rig_id: rig,
                session_id: request.session_id.clone(),
                reported_at_ms: request.reported_at_ms,
                payload: request.status.clone(),
                received_at_ms: now,
            })?;
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

#[derive(Serialize)]
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

/// Each database's count of Active projects, by its ID and file path, read
/// off the request path. A file that cannot be read has no count.
async fn active_projects(paths: Vec<(String, String)>) -> std::collections::HashMap<String, u32> {
    tokio::task::spawn_blocking(move || {
        paths
            .into_iter()
            .filter_map(|(id, path)| {
                let connection =
                    crate::server::database_context::open_scheduler_connection_with_flags(
                        &path,
                        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
                    )
                    .ok()?;
                // Live polls every few seconds: a database another writer
                // holds is left without a count rather than waited on.
                connection
                    .busy_timeout(std::time::Duration::from_millis(250))
                    .ok()?;
                let count: i64 = connection
                    .query_row("SELECT COUNT(*) FROM project WHERE state=1", [], |row| {
                        row.get(0)
                    })
                    .ok()?;
                Some((id, u32::try_from(count).unwrap_or(u32::MAX)))
            })
            .collect()
    })
    .await
    .unwrap_or_default()
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

/// Operator view: every bound rig with its connectivity, newest status,
/// check-in cursors, assignments and pending receipts. Rigs that have
/// reported but lost their database binding are listed too.
pub(super) async fn statuses(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<RigStatusView>>>, Error> {
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let service = enabled(&state)?;
    let instance = service.instance_id;
    let mut active = active_projects(
        catalogs
            .iter()
            .map(|catalog| (catalog.id.clone(), catalog.database_path.clone()))
            .collect(),
    )
    .await;
    let sync_now = now_ms();
    let mut sync: std::collections::HashMap<String, crate::server::sync_sequence::SyncLive> =
        catalogs
            .iter()
            .filter_map(|catalog| {
                crate::server::sync_sequence::SyncLive::for_catalog(&state, catalog, sync_now)
                    .map(|view| (catalog.id.clone(), view))
            })
            .collect();
    let views = service
        .query(move |store| {
            let now = now_ms();
            // Every bound database is a rig; name the database beside it.
            let mut rigs: std::collections::BTreeMap<Uuid, (Option<String>, Option<String>)> =
                std::collections::BTreeMap::new();
            for (identity, catalog) in identified_catalogs(&catalogs, instance).iter() {
                if let Some(binding) = store.catalog_rig(identity.id)? {
                    rigs.insert(
                        binding.rig.id,
                        (Some(catalog.id.clone()), Some(catalog.name.clone())),
                    );
                }
            }
            for status in store.rig_statuses()? {
                rigs.entry(status.rig_id).or_insert((None, None));
            }
            let mut views = Vec::new();
            for (rig_id, (catalog_slug, catalog_name)) in rigs {
                let Some(rig) = store.rig(rig_id)? else {
                    continue;
                };
                let status = store.rig_status(rig_id)?;
                let status_age_ms = status
                    .as_ref()
                    .map(|s| now.saturating_sub(s.received_at_ms));
                let contacts = contacts(store.contacts_for_rig(rig_id)?);
                let sync = catalog_slug.as_ref().and_then(|slug| sync.remove(slug));
                let active_projects = catalog_slug.as_ref().and_then(|slug| active.remove(slug));
                let connectivity = connectivity(
                    &contacts,
                    sync.as_ref().and_then(|sync| sync.last_contact_ms),
                    now,
                );
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
                views.push(RigStatusView {
                    weather: store.rig_weather(rig_id, None, now)?,
                    rig,
                    catalog_slug,
                    catalog_name,
                    status_stale: status_age_ms.is_some_and(|age| {
                        age > status
                            .as_ref()
                            .and_then(|s| s.payload["fresh_for_ms"].as_u64())
                            .unwrap_or(STATUS_FRESH_MS)
                            .clamp(15_000, STATUS_FRESH_MS)
                    }),
                    status_age_ms,
                    status,
                    checkins: store.feed_cursors_for_rig(rig_id)?,
                    contacts,
                    connectivity,
                    assignments,
                    pending_receipts,
                    recent_operations: store.recent_operations(rig_id)?,
                    sync,
                    active_projects,
                });
            }
            views.sort_by(|a, b| a.rig.name.cmp(&b.rig.name).then(a.rig.id.cmp(&b.rig.id)));
            Ok(views)
        })
        .await?;
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
