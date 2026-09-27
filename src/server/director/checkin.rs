//! The plugin's check-in and live status. Check-in stores one page of
//! journaled receipts from one ledger and acknowledges the contiguous cursor;
//! status keeps the newest coalesced report per rig. Neither changes a plan,
//! a rig database or the program; both tell the plugin whether to pull again.

use super::*;
use psf_guard_director_meta::inbox::{Receipt, RigStatus, Stored, MAX_PAGE};
use std::time::{SystemTime, UNIX_EPOCH};

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
pub(super) struct RigStatusView {
    rig: NamedIdentity,
    status: RigStatus,
    /// The last acknowledged cursor per ledger this rig has checked in with.
    checkins: Vec<psf_guard_director_meta::inbox::FeedCursor>,
}

/// Operator view: the newest status and check-in cursors for every rig.
pub(super) async fn statuses(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<RigStatusView>>>, Error> {
    let views = enabled(&state)?
        .run(|store| {
            let mut views = Vec::new();
            for status in store.rig_statuses()? {
                let Some(rig) = store.rig(status.rig_id)? else {
                    continue;
                };
                views.push(RigStatusView {
                    rig,
                    checkins: store.feed_cursors_for_rig(status.rig_id)?,
                    status,
                });
            }
            Ok(views)
        })
        .await?;
    Ok(Json(ApiResponse::success(views)))
}
