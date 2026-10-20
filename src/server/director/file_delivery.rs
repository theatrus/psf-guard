//! Activation delivery through the catalog's existing TS file connection.
//! Uses the same planning merge, durable previews and destination guard as Sync.

use super::{
    activation::{planning_copy, planning_copy_from, Push},
    remote_push::Waiting,
    *,
};
use crate::commands::sync::{sync_planning, PlanningOptions};
use crate::server::{
    api::{SchedulerSyncKind, SchedulerSyncRequest, SchedulerSyncResponse},
    handlers::{execute_scheduler_sync_paths, SyncGuardMode},
    sync_preview::{connection_fingerprint, fingerprint_queries, planning_destination_fingerprint},
};
use anyhow::Context;
use psf_guard_director_meta::activation::{RemotePush, TsSyncDelivery, MAX_PUSH_ERROR_CHARS};
use rusqlite::{params_from_iter, types::Value, Connection, OptionalExtension};
use std::path::PathBuf;

#[derive(Clone, Serialize)]
pub(super) struct Preview {
    pub path: String,
    #[serde(skip)]
    pub local_fingerprint: String,
    #[serde(rename = "revision")]
    pub destination_fingerprint: Option<String>,
    #[serde(skip)]
    result: Option<SchedulerSyncResponse>,
    #[serde(skip)]
    source: Option<Arc<std::sync::Mutex<Connection>>>,
}

/// Remove staged transfers unless a durable activation record owns them.
pub(super) struct Staged {
    state: Arc<AppState>,
    previews: Vec<(String, String)>,
}

impl Staged {
    pub fn new(state: Arc<AppState>) -> Self {
        Self {
            state,
            previews: Vec::new(),
        }
    }
    pub fn track(&mut self, slug: &str, push: &Push) {
        if let Some(id) = push.ts_sync.as_ref().and_then(|d| d.preview_id.as_ref()) {
            self.previews.push((slug.into(), id.clone()));
        }
    }
    pub fn recorded(&mut self, approved: &[String]) {
        self.previews.retain(|(_, id)| !approved.contains(id));
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        for (slug, id) in &self.previews {
            let _ = self.state.sync_previews.discard(id, slug);
        }
    }
}

fn request(slug: &str) -> SchedulerSyncRequest {
    SchedulerSyncRequest {
        peer_db_id: format!("ts:{slug}"),
        kind: SchedulerSyncKind::PushPlanning,
        dry_run: true,
        with_image_data: Some(false),
        project: None,
        target: None,
        status: None,
        reviewed_only: true,
    }
}

/// Bind all pre-existing planning values, including fields activation leaves
/// alone, without binding randomly minted GUIDs on newly created plan rows.
pub(super) fn local_fingerprint(source: &Connection) -> anyhow::Result<String> {
    connection_fingerprint(source, &fingerprint_queries(&request("")))
}

/// Planning sync is additive. Restrict its source to the project activated
/// here so it cannot restore stale copies of unrelated TS projects.
fn projection(source: &Connection, project_guid: &str) -> anyhow::Result<Connection> {
    let copy = planning_copy_from(source).map_err(anyhow::Error::msg)?;
    copy.execute("DELETE FROM exposureplan WHERE targetid IS NULL OR targetid NOT IN (SELECT t.Id FROM target t JOIN project p ON p.Id=t.projectid WHERE p.guid=?1)", [project_guid])?;
    copy.execute("DELETE FROM target WHERE projectid IS NULL OR projectid NOT IN (SELECT Id FROM project WHERE guid=?1)", [project_guid])?;
    copy.execute("DELETE FROM ruleweight WHERE projectid IS NULL OR projectid NOT IN (SELECT Id FROM project WHERE guid=?1)", [project_guid])?;
    copy.execute(
        "DELETE FROM project WHERE guid<>?1 OR guid IS NULL",
        [project_guid],
    )?;
    copy.execute("DELETE FROM exposuretemplate WHERE Id NOT IN (SELECT exposureTemplateId FROM exposureplan WHERE exposureTemplateId IS NOT NULL)", [])?;
    Ok(copy)
}

/// Fields Director owns on an existing project; other TS settings stay live.
pub(super) struct Intent {
    pub active: bool,
    pub reactivate: bool,
    pub scheduling: Vec<String>,
    /// Collaboration adds a floor without lowering a stricter live TS limit.
    pub minimum_altitude: Option<f64>,
}

fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}
fn columns(db: &Connection, table: &str) -> anyhow::Result<Vec<String>> {
    Ok(db
        .prepare(&format!("PRAGMA table_info({table})"))?
        .query_map([], |r| r.get(1))?
        .collect::<rusqlite::Result<_>>()?)
}

fn preserve_ts_fields(
    copy: &Connection,
    ts: &Connection,
    table: &str,
    owned: &[String],
) -> anyhow::Result<()> {
    let present = columns(ts, table)?;
    let keep: Vec<String> = columns(copy, table)?
        .into_iter()
        .filter(|name| {
            present.iter().any(|p| p.eq_ignore_ascii_case(name))
                && !owned.iter().any(|p| p.eq_ignore_ascii_case(name))
        })
        .collect();
    if keep.is_empty() {
        return Ok(());
    }
    let rows = copy
        .prepare(&format!(
            "SELECT Id,guid FROM {table} WHERE guid IS NOT NULL"
        ))?
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let names = keep.iter().map(|c| quoted(c)).collect::<Vec<_>>();
    let mut read = ts.prepare(&format!(
        "SELECT {} FROM {table} WHERE guid=?1",
        names.join(",")
    ))?;
    let mut update = copy.prepare(&format!(
        "UPDATE {table} SET {} WHERE Id=?",
        names
            .iter()
            .map(|c| format!("{c}=?"))
            .collect::<Vec<_>>()
            .join(",")
    ))?;
    for (id, guid) in rows {
        let mut values = read.query([&guid])?;
        if let Some(row) = values.next()? {
            let mut fields = (0..keep.len())
                .map(|i| row.get::<_, Value>(i))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            anyhow::ensure!(
                values.next()?.is_none(),
                "TS has duplicate {table} GUID {guid}; repair it before activating"
            );
            fields.push(Value::Integer(id));
            update.execute(params_from_iter(fields))?;
        }
    }
    Ok(())
}

/// Keep settings activation has never owned: descriptions, priority, ROI,
/// existing template settings and rule weights. The shared Sync engine still
/// applies the resulting projection and guards the TS destination.
fn preserve_settings(
    copy: &Connection,
    ts: &Connection,
    guid: &str,
    intent: &Intent,
) -> anyhow::Result<()> {
    let mut project: Vec<String> = ["Id", "guid", "state", "isMosaic"]
        .into_iter()
        .map(str::to_owned)
        .collect();
    project.extend(intent.scheduling.clone());
    for (table, fields) in [
        ("project", project),
        (
            "target",
            [
                "Id",
                "guid",
                "projectid",
                "ra",
                "dec",
                "rotation",
                "epochcode",
                "active",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ),
        (
            "exposureplan",
            [
                "Id",
                "guid",
                "targetid",
                "exposureTemplateId",
                "exposure",
                "desired",
                "enabled",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
        ),
        (
            "exposuretemplate",
            ["Id", "guid"].into_iter().map(str::to_owned).collect(),
        ),
    ] {
        preserve_ts_fields(copy, ts, table, &fields)?;
    }
    if let Some(minimum) = intent.minimum_altitude {
        copy.execute("UPDATE project SET minimumaltitude=?2 WHERE guid=?1 AND COALESCE(minimumaltitude,0)<?2", rusqlite::params![guid, minimum])?;
    }
    if let Some((ts_id, state)) = ts
        .query_row(
            "SELECT Id,COALESCE(state,0) FROM project WHERE guid=?1",
            [guid],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)),
        )
        .optional()?
    {
        let state = if intent.active {
            if state == 0 || intent.reactivate && state == 2 {
                1
            } else {
                state
            }
        } else if state == 0 || state == 1 {
            2
        } else {
            state
        };
        copy.execute(
            "UPDATE project SET state=?2 WHERE guid=?1",
            rusqlite::params![guid, state],
        )?;
        let local_id: i64 =
            copy.query_row("SELECT Id FROM project WHERE guid=?1", [guid], |r| r.get(0))?;
        copy.execute("DELETE FROM ruleweight", [])?;
        let local_columns = columns(copy, "ruleweight")?;
        let fields: Vec<String> = columns(ts, "ruleweight")?
            .into_iter()
            .filter(|c| {
                !c.eq_ignore_ascii_case("Id")
                    && local_columns.iter().any(|p| p.eq_ignore_ascii_case(c))
            })
            .collect();
        let mut read = ts.prepare(&format!(
            "SELECT {} FROM ruleweight WHERE projectid=?1",
            fields
                .iter()
                .map(|c| quoted(c))
                .collect::<Vec<_>>()
                .join(",")
        ))?;
        let mut rows = read.query([ts_id])?;
        let mut insert = copy.prepare(&format!(
            "INSERT INTO ruleweight ({}) VALUES ({})",
            fields
                .iter()
                .map(|c| quoted(c))
                .collect::<Vec<_>>()
                .join(","),
            vec!["?"; fields.len()].join(",")
        ))?;
        while let Some(row) = rows.next()? {
            let values = fields
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    if c.eq_ignore_ascii_case("projectid") {
                        Ok(Value::Integer(local_id))
                    } else {
                        row.get::<_, Value>(i)
                    }
                })
                .collect::<rusqlite::Result<Vec<_>>>()?;
            insert.execute(params_from_iter(values))?;
        }
    }
    Ok(())
}

pub(super) fn shot_template(
    push: &Push,
    id: i64,
) -> Option<(String, String, Option<i64>, Option<i64>)> {
    let source = push.ts_file.as_ref()?.source.as_ref()?.lock().ok()?;
    source.query_row("SELECT t.name,t.filtername,t.gain,t.offset FROM exposureplan e JOIN exposuretemplate t ON t.Id=e.exposureTemplateId WHERE e.Id=?1", [id], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).ok()
}

fn counts(result: &SchedulerSyncResponse) -> BTreeMap<String, i64> {
    let mut counts = BTreeMap::new();
    for (name, table) in [
        ("project", &result.project),
        ("target", &result.target),
        ("exposuretemplate", &result.exposuretemplate),
        ("exposureplan", &result.exposureplan),
        ("ruleweight", &result.ruleweight),
    ] {
        counts.insert(format!("{name}_inserted"), table.inserted as i64);
        counts.insert(format!("{name}_updated"), table.updated as i64);
    }
    counts
}

pub(super) fn prepare(
    context: &DatabaseContext,
    source: &Connection,
    before: String,
    project_guid: &str,
    intent: Intent,
    exposures: &[(i64, f64)],
) -> Option<Push> {
    let settings = context.ts_sync.as_ref()?;
    let endpoint = format!("ts:{}", context.id);
    let mut push = Push {
        peer_id: endpoint.clone(),
        peer_name: format!("Target Scheduler ({})", context.name),
        applied: false,
        summary: BTreeMap::new(),
        error: None,
        ts_file: Some(Preview {
            path: settings.source_path.clone(),
            local_fingerprint: before,
            destination_fingerprint: None,
            result: None,
            source: None,
        }),
        ts_sync: Some(TsSyncDelivery {
            source_path: settings.source_path.clone(),
            preview_id: None,
        }),
    };
    let preview = || -> anyhow::Result<(SchedulerSyncResponse, String, Connection)> {
        settings.validate(FilePath::new(&context.database_path))?;
        // Read only the small planning tables in one transaction. The live
        // TS file is never opened for writing by an activation preview.
        let destination = planning_copy(&settings.source_path).map_err(anyhow::Error::msg)?;
        let source = projection(source, project_guid)?;
        let fingerprint = planning_destination_fingerprint(&source, &destination)?;
        preserve_settings(&source, &destination, project_guid, &intent)?;
        for (id, exposure) in exposures {
            source.execute(
                "UPDATE exposureplan SET exposure=?2 WHERE Id=?1",
                rusqlite::params![id, exposure],
            )?;
        }
        let summary = sync_planning(
            &source,
            &destination,
            &PlanningOptions {
                dry_run: true,
                project_filter: None,
            },
        )?;
        let result = SchedulerSyncResponse {
            kind: SchedulerSyncKind::PushPlanning,
            dry_run: true,
            source_db_id: context.id.clone(),
            destination_db_id: endpoint,
            exposuretemplate: (&summary.exposuretemplate).into(),
            project: (&summary.project).into(),
            ruleweight: (&summary.ruleweight).into(),
            target: (&summary.target).into(),
            exposureplan: (&summary.exposureplan).into(),
            acquiredimage: None,
            imagedata: None,
            grades: None,
            calibration: None,
            grade_filled: 0,
            grade_preserved: 0,
            adopted: 0,
            imagedata_bytes: 0,
            total_inserted: summary.total_inserted(),
            total_updated: summary.total_updated(),
            changes: summary.changes.into_iter().take(200).collect(),
        };
        Ok((result, fingerprint, source))
    };
    match preview() {
        Ok((result, fingerprint, source)) => {
            push.summary = counts(&result);
            let file = push.ts_file.as_mut().unwrap();
            file.destination_fingerprint = Some(fingerprint);
            file.result = Some(result);
            file.source = Some(Arc::new(std::sync::Mutex::new(source)));
        }
        Err(error) => push.error = Some(format!("TS planning could not be previewed: {error:#}. The plan can be activated locally; preview and activate again before sending it to TS.")),
    }
    Some(push)
}

/// Freeze the actual projected rows only after the activation digest matches.
/// A preview/check creates no files; Apply stages this before committing locally.
pub(super) fn stage(state: &AppState, context: &str, push: &mut Push) -> anyhow::Result<()> {
    let Some(file) = &push.ts_file else {
        return Ok(());
    };
    let (Some(result), Some(fingerprint)) = (&file.result, &file.destination_fingerprint) else {
        return Ok(());
    };
    let copy = file
        .source
        .as_ref()
        .context("Missing approved TS projection")?
        .lock()
        .map_err(|_| anyhow::anyhow!("Planning projection lock was poisoned"))?;
    let snapshot = state.sync_previews.create_empty_source_snapshot()?;
    let store = || -> anyhow::Result<String> {
        let path = state
            .sync_previews
            .source_snapshot_path_for_file(&snapshot)?;
        copy.backup("main", &path, None)?;
        let record = state.sync_previews.store_activation(
            context.into(),
            request(context),
            snapshot.clone(),
            fingerprint.clone(),
            result.clone(),
        )?;
        Ok(record.id)
    };
    match store() {
        Ok(id) => push.ts_sync.as_mut().unwrap().preview_id = Some(id),
        Err(error) => {
            state.sync_previews.remove_source_snapshot(&snapshot);
            return Err(error);
        }
    }
    Ok(())
}

pub(super) fn is_current(context: &DatabaseContext, delivery: &TsSyncDelivery) -> bool {
    context
        .ts_sync
        .as_ref()
        .is_some_and(|s| s.source_path == delivery.source_path)
}

/// Retry the frozen, approved transfer, never a new blind planning export.
pub(super) async fn send(
    state: &Arc<AppState>,
    service: &Arc<Service>,
    context: &Arc<DatabaseContext>,
    waiting: Waiting,
    delivery: TsSyncDelivery,
) -> Push {
    send_inner(state, service, context, waiting, delivery, false).await
}

async fn send_inner(
    state: &Arc<AppState>,
    service: &Arc<Service>,
    context: &Arc<DatabaseContext>,
    waiting: Waiting,
    delivery: TsSyncDelivery,
    automatic: bool,
) -> Push {
    let _guard = state.sync_apply_lock.lock().await;
    let mut push = Push {
        peer_id: format!("ts:{}", context.id),
        peer_name: format!("Target Scheduler ({})", context.name),
        applied: false,
        summary: BTreeMap::new(),
        error: None,
        ts_file: Some(Preview {
            path: delivery.source_path.clone(),
            local_fingerprint: String::new(),
            destination_fingerprint: None,
            result: None,
            source: None,
        }),
        ts_sync: Some(delivery.clone()),
    };
    let send = async {
        if automatic && (state.shutdown.is_stopping() || state.ts_sync.stopped(&context.id)) {
            return Err("TS planning delivery was stopped.".into());
        }
        let current = service.clone().query(move |store| Ok(store.activation(waiting.project)?.is_some_and(|a| a.revision == waiting.revision))).await
            .map_err(|e| format!("Could not check the activation: {e:?}"))?;
        if !current { return Err("A newer activation replaced this delivery. Refresh the plan.".into()); }
        let connected = state.get_database(&context.id).filter(|c| is_current(c, &delivery))
            .ok_or("This TS connection was removed or changed. Preview and activate again.")?;
        let preview_id = delivery.preview_id.as_ref().ok_or("TS was unavailable in the activation preview. Preview and activate again before sending planning.")?;
        let record = state.sync_previews.get(preview_id).map_err(|e| e.to_string())?
            .filter(|r| r.activation && r.local_db_id == context.id && r.request.peer_db_id == push.peer_id && matches!(r.request.kind, SchedulerSyncKind::PushPlanning))
            .ok_or("The approved TS transfer expired or was removed. Preview and activate again.")?;
        let source = state.sync_previews.source_snapshot_path(&record).map_err(|e| e.to_string())?;
        let destination = PathBuf::from(&connected.ts_sync.as_ref().unwrap().source_path);
        let run = |dry_run, guard| {
            let mut request = record.request.clone(); request.dry_run = dry_run;
            execute_scheduler_sync_paths(state, source.clone(), destination.clone(), context.id.clone(), push.peer_id.clone(), request, guard)
        };
        // Recover a TS commit whose receipt was interrupted. A no-op comparison
        // acknowledges matching rows without overwriting anything TS changed.
        let (comparison, _) = run(true, SyncGuardMode::PlanningPreview { stop: crate::concurrency::Stop::new() }).await.map_err(sync_error)?;
        if comparison.total_inserted == 0 && comparison.total_updated == 0 { return Ok(comparison); }
        run(false, SyncGuardMode::PlanningApply { destination_fingerprint: record.destination_fingerprint }).await
            .map(|(result, _)| result).map_err(|e| format!("{} Preview and activate again to review the current TS planning.", sync_error(e)))
    }.await;
    match send {
        Ok(result) => {
            push.applied = true;
            push.summary = counts(&result);
        }
        Err(error) => push.error = Some(error),
    }
    let receipt = RemotePush {
        rig_id: waiting.rig,
        peer_id: push.peer_id.clone(),
        peer_name: push
            .peer_name
            .chars()
            .take(psf_guard_director_meta::activation::MAX_PEER_NAME_CHARS)
            .collect(),
        applied: push.applied,
        attempted_at_ms: Some(super::activation::now_ms()),
        error: push
            .error
            .as_ref()
            .map(|e| e.chars().take(MAX_PUSH_ERROR_CHARS).collect()),
        ts_sync: Some(delivery.clone()),
    };
    let recorded = service
        .clone()
        .with_writer(move |store| {
            Ok::<_, Error>(store.record_pushes(waiting.project, waiting.revision, &[receipt])?)
        })
        .await;
    if matches!(recorded, Ok(true))
        && push.applied
        && let Some(id) = delivery.preview_id
    {
        let _ = state.sync_previews.discard(&id, &context.id);
    }
    if let Err(error) = recorded {
        tracing::warn!(?error, "Could not record TS activation delivery");
    }
    push
}

fn sync_error(error: crate::server::handlers::AppError) -> String {
    use crate::server::handlers::AppError;
    match error {
        AppError::DatabaseError(message)
        | AppError::BadRequest(message)
        | AppError::Conflict(message)
        | AppError::Forbidden(message)
        | AppError::InternalError(message)
        | AppError::NotFoundMessage(message) => message,
        _ => "The TS planning transfer could not be completed.".into(),
    }
}

/// The ordinary TS sync job retries approved deliveries on its existing
/// on-open/change/schedule settings. It never grants approval to a new export.
pub(super) async fn retry_pending(state: &Arc<AppState>, slug: &str) -> anyhow::Result<()> {
    let Ok(service) = writable(state) else {
        return Ok(());
    };
    let Some(context) = state.get_database(slug).filter(|c| c.ts_sync.is_some()) else {
        return Ok(());
    };
    let endpoint = format!("ts:{slug}");
    let catalog = identity_of(service.instance_id, &context.database_path).id;
    let pending = service
        .clone()
        .query(move |store| {
            let mut pending = Vec::new();
            for activation in store.pending_pushes(&endpoint)? {
                for push in &activation.pushes {
                    if !push.applied
                        && push.peer_id == endpoint
                        && remote_push::catalog_of(&activation, push.rig_id) == Some(catalog)
                        && let Some(delivery) = &push.ts_sync
                        && delivery.preview_id.is_some()
                    {
                        pending.push((
                            Waiting {
                                project: activation.project_id,
                                revision: activation.revision,
                                rig: push.rig_id,
                            },
                            delivery.clone(),
                        ));
                    }
                }
            }
            Ok(pending)
        })
        .await
        .map_err(|e| anyhow::anyhow!("Reading pending TS planning deliveries: {e:?}"))?;
    let mut failures = Vec::new();
    for (waiting, delivery) in pending {
        if state.shutdown.is_stopping() || state.ts_sync.stopped(slug) {
            break;
        }
        let push = send_inner(state, &service, &context, waiting, delivery, true).await;
        if !push.applied {
            failures.push(push.error.unwrap_or_default());
        }
    }
    anyhow::ensure!(
        failures.is_empty(),
        "TS planning delivery is pending: {}",
        failures.join("; ")
    );
    Ok(())
}
