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
    let service = enabled(&state)?;
    if request.coordinator_instance_id != service.instance_id {
        return Err(Error::WrongRig.into());
    }
    if request.events.is_empty() || request.events.len() > MAX_PAGE {
        return Err(Error::Invalid.into());
    }
    let receipts = request
        .events
        .iter()
        .map(|event| receipt_from(event, rig, &request.ledger_id))
        .collect::<Result<Vec<_>, Error>>()?;
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let metadata_permit = admit(&service.admission).await?;
    let catalog_permit = admit(&service.discovery_admission).await?;
    let ack = tokio::task::spawn_blocking(move || {
        let _permits = (metadata_permit, catalog_permit);
        let mut store = service.store.lock().map_err(|_| Error::Internal)?;
        store
            .catalog_rig(request.catalog_id)?
            .filter(|binding| binding.rig.id == rig)
            .ok_or(Error::WrongRig)?;
        let now = now_ms();
        let (outcomes, cursor) = store.store_receipts(&receipts, now)?;
        store.record_contact(rig, ContactKind::CheckIn, now, Some(&request.ledger_id))?;
        let program_revision = program::current_revision(
            &store,
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
                .zip(&receipts)
                .filter(|(o, _)| **o == Stored::Conflict)
                .map(|(_, r)| r.sequence)
                .collect(),
            outcomes,
            program_revision,
            program_changed,
            received_at_ms: now,
        })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director check-in worker failed");
        Error::Internal
    })??;
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
    let metadata_permit = admit(&service.admission).await?;
    let catalog_permit = admit(&service.discovery_admission).await?;
    let answer = tokio::task::spawn_blocking(move || {
        let _permits = (metadata_permit, catalog_permit);
        let mut store = service.store.lock().map_err(|_| Error::Internal)?;
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
            &store,
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
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director status worker failed");
        Error::Internal
    })??;
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
    rig: NamedIdentity,
    catalog_slug: Option<String>,
    catalog_name: Option<String>,
    /// The newest coalesced report, or `None` before the first.
    status: Option<RigStatus>,
    status_age_ms: Option<u64>,
    /// The report is older than ten minutes; show it as history, not as now.
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

fn connectivity(contacts: &Contacts, now: u64) -> Connectivity {
    let last = [&contacts.program_pull, &contacts.check_in, &contacts.status]
        .into_iter()
        .flatten()
        .map(|c| c.at_ms)
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
    let views = enabled(&state)?
        .run(move |store| {
            let now = now_ms();
            // Every bound database is a rig; name the database beside it.
            let mut rigs: std::collections::BTreeMap<Uuid, (Option<String>, Option<String>)> =
                std::collections::BTreeMap::new();
            for (identity, catalog) in identified_catalogs(&catalogs).iter() {
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
                let connectivity = connectivity(&contacts, now);
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
                    rig,
                    catalog_slug,
                    catalog_name,
                    status_stale: status_age_ms.is_some_and(|age| age > STATUS_FRESH_MS),
                    status_age_ms,
                    status,
                    checkins: store.feed_cursors_for_rig(rig_id)?,
                    contacts,
                    connectivity,
                    assignments,
                    pending_receipts,
                });
            }
            views.sort_by(|a, b| a.rig.name.cmp(&b.rig.name).then(a.rig.id.cmp(&b.rig.id)));
            Ok(views)
        })
        .await?;
    Ok(Json(ApiResponse::success(views)))
}
