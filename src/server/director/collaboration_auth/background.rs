//! Opt-in, rig-bound request refresh. One loop owns all connections; network
//! waits never hold the metadata writer or grant equipment authority.
use super::*;
use psf_guard_director_meta::{
    collaboration::ImportAction, collaboration_connection::BackgroundPolicy,
};

pub(super) struct Record {
    policy: BackgroundPolicy,
    due: Instant,
    status: Status,
    failures: u32,
    reports_due: Instant,
}

#[derive(Clone, Default, Serialize)]
pub(super) struct Status {
    running: bool,
    last_started_ms: Option<u64>,
    last_success_ms: Option<u64>,
    next_run_ms: Option<u64>,
    last_error: Option<&'static str>,
    result: Option<PullResult>,
    reports: Option<workflows::evidence::AutomaticResult>,
    report_error: Option<&'static str>,
}

#[derive(Clone, Default, Serialize)]
pub(super) struct PullResult {
    night: String,
    imported: usize,
    unchanged: usize,
    activated: usize,
    held: Vec<String>,
}

pub(crate) fn spawn(state: Arc<AppState>) {
    if state.director.is_none() {
        return;
    }
    let weak = Arc::downgrade(&state);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tick.tick().await;
            let Some(state) = weak.upgrade() else {
                break;
            };
            if let Err(error) = sweep(&state).await {
                tracing::warn!(reason = error.1, "Collaboration background refresh failed");
            }
        }
    });
}

pub(super) async fn sweep(state: &Arc<AppState>) -> Result<(), Failure> {
    if !state.database_management_allowed() {
        return Ok(());
    }
    let service = writable(state)?;
    let ids = service
        .clone()
        .query(|s| s.collaboration_connection_ids())
        .await?;
    if let Ok(mut records) = service.collaboration.background.lock() {
        records.retain(|id, _| ids.contains(id));
    }
    for id in ids {
        // Interactive setup/check-in wins; do not queue automatic work behind it.
        let Ok(_gate) = service.collaboration.gate.try_lock() else {
            break;
        };
        let b = match binding(service.clone(), id).await {
            Ok(binding) => binding,
            Err(error) => {
                tracing::warn!(connection_id=%id, reason=error.1, "Collaboration binding could not be loaded");
                continue;
            }
        };
        let Some(policy) = b
            .background
            .as_ref()
            .filter(|p| p.enabled || p.automatic_reports)
        else {
            continue;
        };
        if b.state != ConnectionState::Registered {
            continue;
        }
        let due = service
            .collaboration
            .background
            .lock()
            .map_err(|_| invalid())?
            .get(&id)
            .is_none_or(|r| r.policy != *policy || Instant::now() >= r.due);
        if policy.enabled
            && due
            && let Err(error) = run(state, service.clone(), b.clone(), false).await
        {
            tracing::warn!(connection_id=%id, reason=error.1, "Collaboration request refresh held; existing plans retained");
            if error.0 == StatusCode::UNAUTHORIZED {
                continue;
            }
        }
        if policy.automatic_reports {
            let report_due = service
                .collaboration
                .background
                .lock()
                .map_err(|_| invalid())?
                .get(&id)
                .is_none_or(|r| Instant::now() >= r.reports_due);
            if report_due && let Err(error) = run_reports(state, service.clone(), &b).await {
                tracing::warn!(connection_id=%id, reason=error.1, "Automatic collaboration reports held; outbox retained");
            }
        }
    }
    Ok(())
}

pub(super) async fn run_reports(
    state: &Arc<AppState>,
    service: Arc<Service>,
    b: &ConnectionBinding,
) -> Result<(), Failure> {
    if b.state != ConnectionState::Registered {
        return Err(Failure(
            StatusCode::CONFLICT,
            "Reconnect before submitting reports",
        ));
    }
    let policy = b
        .background
        .as_ref()
        .filter(|p| p.automatic_reports)
        .ok_or_else(invalid)?;
    let outcome = workflows::evidence::automatic(state, service.clone(), b).await;
    let mut records = service
        .collaboration
        .background
        .lock()
        .map_err(|_| invalid())?;
    let r = records
        .entry(b.id)
        .or_insert_with(|| Record::new(policy.clone()));
    r.reports_due = Instant::now() + Duration::from_secs(300);
    match outcome {
        Ok(result) => {
            r.status.reports = Some(result);
            r.status.report_error = None;
            Ok(())
        }
        Err(error) => {
            r.status.report_error = Some(error.1);
            Err(error)
        }
    }
}

pub(super) async fn catalogs(
    state: &AppState,
    service: Arc<Service>,
    rig: Uuid,
) -> Result<Vec<(CatalogIdentity, Arc<DatabaseContext>)>, Failure> {
    let contexts = state.all_databases();
    service
        .clone()
        .with_reader(move |store| {
            let found = identified_catalogs(&contexts, service.instance_id);
            let mut result = Vec::new();
            for (identity, context) in found.iter() {
                if store
                    .catalog_rig(identity.id)?
                    .is_some_and(|r| r.rig.id == rig)
                {
                    result.push((*identity, context.clone()));
                }
            }
            Ok::<_, Error>(result)
        })
        .await
        .map_err(Into::into)
}

pub(super) async fn status(
    state: &AppState,
    service: Arc<Service>,
    b: &ConnectionBinding,
) -> Result<Value, Failure> {
    let catalogs = catalogs(state, service.clone(), b.rig_id)
        .await?
        .into_iter()
        .map(|(id, c)| json!({"id":id.id,"slug":c.id,"name":c.name}))
        .collect::<Vec<_>>();
    let id = b.id;
    let projects = service
        .clone()
        .query(move |s| s.collaboration_connection_projects(id))
        .await?
        .into_iter()
        .map(|(id, name, project)| json!({"id":id,"name":name,"project_id":project}))
        .collect::<Vec<_>>();
    let status = service
        .collaboration
        .background
        .lock()
        .map_err(|_| invalid())?
        .get(&id)
        .map(|r| r.status.clone())
        .unwrap_or_default();
    let connection = view(registry(state)?, b.clone()).await?;
    Ok(
        json!({"policy":b.background,"status":status,"catalogs":catalogs,"projects":projects,"connection_status":connection["status"]}),
    )
}

pub(super) async fn configure(
    state: &AppState,
    service: Arc<Service>,
    b: ConnectionBinding,
    expected: Option<BackgroundPolicy>,
    policy: Option<BackgroundPolicy>,
) -> Result<Value, Failure> {
    if b.background != expected {
        return Err(Failure(
            StatusCode::CONFLICT,
            "Background policy changed; reload before saving",
        ));
    }
    if let Some(p) = &policy {
        p.validate().map_err(|_| invalid())?;
        if p.enabled || p.automatic_reports {
            if b.state != ConnectionState::Registered || b.settings.is_none() {
                return Err(Failure(
                    StatusCode::CONFLICT,
                    "Connect and configure this rig before enabling background refresh",
                ));
            }
            resolve(state, service.clone(), &b, p).await?;
            let id = b.id;
            let projects = service
                .clone()
                .query(move |s| s.collaboration_connection_projects(id))
                .await?;
            if p.project_ids
                .iter()
                .any(|id| !projects.iter().any(|(remote, _, _)| remote == id))
            {
                let path = registry(state)?;
                let copy = b.clone();
                let token = file(move || credentials::read(&path, &copy))
                    .await?
                    .ok_or(Failure(
                        StatusCode::CONFLICT,
                        "Reconnect before allowing a new project",
                    ))?;
                let remote = Remote::new(&b)?;
                let bytes = remote.get("agent/projects", Some(&token), None).await?;
                let joined =
                    astrocollab::decode_projects(&bytes, &b.source().map_err(Error::from)?)
                        .map_err(|_| invalid())?;
                if p.project_ids.iter().any(|id| {
                    !projects.iter().any(|(remote, _, _)| remote == id)
                        && !joined
                            .projects
                            .iter()
                            .any(|project| project.project_id == *id && project.joined)
                }) {
                    return Err(Failure(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Join each allowed project before enabling its background refresh",
                    ));
                }
            }
        }
    }
    let mut new = b.clone();
    new.background = policy;
    let copy = new.clone();
    service
        .clone()
        .run(move |s| s.update_collaboration_connection(&b, &copy))
        .await?;
    service
        .collaboration
        .background
        .lock()
        .map_err(|_| invalid())?
        .remove(&new.id);
    status(state, service, &new).await
}

async fn resolve(
    state: &AppState,
    service: Arc<Service>,
    b: &ConnectionBinding,
    policy: &BackgroundPolicy,
) -> Result<
    (
        psf_guard_director_core::visibility::Site,
        Arc<DatabaseContext>,
    ),
    Failure,
> {
    let catalogs = catalogs(state, service.clone(), b.rig_id).await?;
    if catalogs.len() != 1 {
        return Err(Failure(
            StatusCode::CONFLICT,
            "This rig must have exactly one available database for automation",
        ));
    }
    let catalog = catalogs
        .into_iter()
        .find(|(id, _)| id.id == policy.catalog_id)
        .ok_or(Failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The selected database is missing, duplicated or no longer belongs to this rig",
        ))?
        .1;
    let rig = b.rig_id;
    let site = service
        .query(move |s| {
            let profile = s.rig_profile(rig)?;
            Ok(s.rig_site(rig, profile.as_ref())?.location)
        })
        .await?
        .ok_or(Failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Set this rig's observing site before enabling background refresh",
        ))?;
    Ok((site, catalog))
}

pub(super) async fn run(
    state: &Arc<AppState>,
    service: Arc<Service>,
    b: ConnectionBinding,
    force: bool,
) -> Result<PullResult, Failure> {
    if b.state != ConnectionState::Registered {
        return Err(Failure(
            StatusCode::CONFLICT,
            "Reconnect before refreshing work",
        ));
    }
    let policy = b.background.clone().filter(|p| p.enabled).ok_or(Failure(
        StatusCode::CONFLICT,
        "Background refresh is disabled",
    ))?;
    let now = workflows::now_ms()?;
    {
        let mut records = service
            .collaboration
            .background
            .lock()
            .map_err(|_| invalid())?;
        let r = records
            .entry(b.id)
            .or_insert_with(|| Record::new(policy.clone()));
        if r.policy != policy {
            r.policy = policy.clone();
            r.failures = 0;
        }
        r.status.running = true;
        r.status.last_started_ms = Some(now);
    }
    let mut result = async {
        let (site, _) = resolve(state, service.clone(), &b, &policy).await?;
        let night = workflows::resolve_night(service.clone(), b.rig_id, None, None).await?;
        let id = b.id;
        let completed = service
            .clone()
            .query(move |s| s.collaboration_nightly_run(id))
            .await?;
        if !force
            && completed
                .as_ref()
                .is_some_and(|(date, _)| date == &night.night)
        {
            return Ok((
                PullResult {
                    night: night.night,
                    ..Default::default()
                },
                next_night_seconds(site, now),
                completed.map(|(_, at)| at).unwrap_or(now),
            ));
        }
        let pulled = pull(state, service.clone(), &b, &policy, now).await?;
        let date = pulled.night.clone();
        service
            .clone()
            .run(move |s| s.complete_collaboration_night(id, &date, now))
            .await?;
        Ok::<_, Failure>((pulled, next_night_seconds(site, now), now))
    }
    .await;
    if matches!(&result, Err(Failure(StatusCode::UNAUTHORIZED, _)))
        && let Err(error) =
            change(service.clone(), b.clone(), ConnectionState::Rejected, None).await
    {
        result = Err(error);
    }
    let finished = workflows::now_ms().unwrap_or(now);
    if let Ok(mut records) = service.collaboration.background.lock()
        && let Some(r) = records.get_mut(&b.id)
    {
        r.status.running = false;
        match &result {
            Ok((value, _, at)) => {
                r.failures = 0;
                r.status.last_success_ms = Some(*at);
                r.status.last_error = None;
                r.status.result = Some(value.clone());
            }
            Err(error) => {
                r.failures = (r.failures + 1).min(6);
                r.status.last_error = Some(error.1);
            }
        }
        let delay = result
            .as_ref()
            .map(|(_, delay, _)| {
                delay
                    .saturating_sub(finished.saturating_sub(now) / 1000)
                    .max(1)
            })
            .unwrap_or_else(|_| retry_seconds(&policy, r.failures));
        r.due = Instant::now() + Duration::from_secs(delay);
        r.status.next_run_ms = Some(finished.saturating_add(delay * 1000));
    }
    result.map(|(value, _, _)| value)
}

impl Record {
    fn new(policy: BackgroundPolicy) -> Self {
        Self {
            policy,
            due: Instant::now(),
            reports_due: Instant::now(),
            status: Status::default(),
            failures: 0,
        }
    }
}

fn next_night_seconds(site: psf_guard_director_core::visibility::Site, now: u64) -> u64 {
    let offset = (site.longitude_degrees / 15.0 * 3_600_000.0).round() as i64;
    let local = now as i64 + offset;
    let next = (local - 43_200_000).div_euclid(86_400_000) * 86_400_000 + 129_600_000;
    ((next - local) as u64).div_ceil(1000).max(1)
}

fn retry_seconds(policy: &BackgroundPolicy, failures: u32) -> u64 {
    (u64::from(policy.interval_minutes) * 60 * (1u64 << failures.min(6))).min(86400)
}

async fn pull(
    state: &Arc<AppState>,
    service: Arc<Service>,
    b: &ConnectionBinding,
    policy: &BackgroundPolicy,
    now: u64,
) -> Result<PullResult, Failure> {
    let (site, catalog) = resolve(state, service.clone(), b, policy).await?;
    let night = blocking(move || psf_guard_director_interop::workflow::Night::for_site(site, now))
        .await?
        .map_err(|_| {
            Failure(
                StatusCode::UNPROCESSABLE_ENTITY,
                "No astronomical observing night is available for this rig's site",
            )
        })?;
    let path = registry(state)?;
    let copy = b.clone();
    let token = file(move || credentials::read(&path, &copy))
        .await?
        .ok_or(Failure(
            StatusCode::CONFLICT,
            "Credential missing; existing plans retained",
        ))?;
    let remote = Remote::new(b)?;
    let source = b.source().map_err(Error::from)?;
    workflows::hello(service.clone(), b, &remote, &token).await?;
    let bytes = workflows::tonight(&remote, &token, &night).await?;
    let work = astrocollab::decode_tonight(&bytes, &source, &night.night).map_err(|_| invalid())?;
    let mut result = PullResult {
        night: night.night.clone(),
        ..Default::default()
    };
    for share in work
        .shares
        .iter()
        .filter(|s| policy.project_ids.contains(&s.project_id))
    {
        let plan = match psf_guard_director_interop::collaboration::prepare_import(
            &bytes,
            &source,
            &night.night,
            &share.task_id,
        ) {
            Ok(plan) => plan,
            Err(_) => {
                result
                    .held
                    .push(format!("{}: assignment needs review", share.task_id));
                continue;
            }
        };
        let rig = b.rig_id;
        let copy = plan.clone();
        let imported = service
            .clone()
            .run(move |s| {
                let preview = s.preview_collaboration_import(&copy, rig)?;
                s.apply_collaboration_import(&copy, rig, &preview.review_digest, now)?;
                Ok(preview.action)
            })
            .await;
        match imported {
            Ok(ImportAction::Unchanged) => result.unchanged += 1,
            Ok(_) => result.imported += 1,
            Err(_) => {
                result
                    .held
                    .push(format!("{}: import changed or needs review", share.task_id));
                continue;
            }
        }
        if policy.activate {
            if seed_recipes(service.clone(), b, &plan, catalog.clone(), now)
                .await
                .is_err()
            {
                result.held.push(format!(
                    "{}: choose unambiguous rig recipes in Planning",
                    share.task_id
                ));
                continue;
            }
            let visit = collaboration_activation::Visit {
                import_id: plan.import_id(),
                source_digest: plan.digest().into(),
            };
            match activation::background_apply(
                state.clone(),
                plan.project_id(),
                rig,
                policy.catalog_id,
                visit,
            )
            .await
            {
                Ok(true) => result.activated += 1,
                Ok(false) => {}
                Err(activation::ActivationError::NotReady(message)) => {
                    result.held.push(format!("{}: {message}", share.task_id))
                }
                Err(_) => result.held.push(format!(
                    "{}: activation failed; review the plan",
                    share.task_id
                )),
            }
        }
    }
    Ok(result)
}

async fn seed_recipes(
    service: Arc<Service>,
    b: &ConnectionBinding,
    imported: &psf_guard_director_interop::collaboration::PreparedImport,
    catalog: Arc<DatabaseContext>,
    now: u64,
) -> Result<(), Failure> {
    use psf_guard_director_meta::plan::{Contribution, TemplateChoice};
    let project = imported.project_id();
    let mut draft = service
        .clone()
        .query(move |s| s.plan_draft(project))
        .await?
        .ok_or_else(invalid)?;
    // Never reconstruct an operator-edited or deliberately disabled plan.
    if draft.revision != 1 || !draft.contributions.is_empty() {
        return Ok(());
    }
    let templates = blocking(move || {
        let conn = crate::server::database_context::open_scheduler_connection_with_flags(
            &catalog.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(StoreError::from)
        .map_err(Error::from)?;
        conn.busy_timeout(Duration::from_secs(2))
            .map_err(StoreError::from)?;
        plan::read_templates(&conn)
    })
    .await??
    .usable;
    let settings = b.settings.as_ref().ok_or_else(invalid)?;
    let mut recipes = BTreeMap::new();
    for demand in &imported.share().demands {
        let mut choices = templates
            .iter()
            .filter(|t| {
                t.guid.is_some()
                    && t.bin.unwrap_or(1) == settings.binning as i32
                    && astrocollab::fold_filter(&t.filter_name).ok().as_deref()
                        == Some(demand.filter.as_str())
            })
            .collect::<Vec<_>>();
        if choices.len() > 1 {
            choices.retain(|t| (t.default_exposure * 1000.0).round() as u64 == demand.exposure_ms);
        }
        let [template] = choices.as_slice() else {
            return Err(invalid());
        };
        let profile = settings
            .filters
            .iter()
            .find(|(label, _)| {
                astrocollab::fold_filter(label).ok().as_deref() == Some(demand.filter.as_str())
            })
            .ok_or_else(invalid)?
            .1;
        if (profile.exposure_seconds * 1000.0).round() as u64 != demand.exposure_ms {
            return Err(invalid());
        }
        let objective = Uuid::new_v5(&project, demand.filter.as_bytes());
        if !draft.objectives.iter().any(|o| o.id == objective) {
            return Err(invalid());
        }
        recipes.entry(objective).or_insert(Contribution {
            id: Uuid::new_v5(&b.rig_id, objective.as_bytes()),
            objective_id: objective,
            rig_id: b.rig_id,
            template: TemplateChoice {
                template_guid: template.guid,
                template_id: Some(template.id),
                name: template.name.clone(),
                filter_name: template.filter_name.clone(),
                gain: template.gain,
                offset: template.offset,
                bin: template.bin,
                readout_mode: template.readout_mode,
                moon: Some(template.moon.clone()),
            },
            exposure_seconds: demand.exposure_ms as f64 / 1000.0,
            panel_ids: vec![],
            enabled: true,
            goal: None,
        });
    }
    if recipes.is_empty() {
        return Err(invalid());
    }
    draft.contributions = recipes.into_values().collect();
    draft.updated_at_ms = now;
    service
        .run(move |s| s.save_plan_draft(&draft, 1).map(|_| ()))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn successful_pulls_wait_until_the_next_rig_local_noon() {
        let site = psf_guard_director_core::visibility::Site {
            latitude_degrees: 35.0,
            longitude_degrees: -105.0,
            elevation_meters: 2000.0,
        };
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-08T03:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(next_night_seconds(site, now), 16 * 3600);
        assert_eq!(next_night_seconds(site, now + 16 * 3_600_000), 24 * 3600);
    }
    #[test]
    fn retries_back_off_and_remain_bounded() {
        let p = BackgroundPolicy {
            enabled: true,
            catalog_id: Uuid::new_v4(),
            project_ids: vec!["000000000002".into()],
            interval_minutes: 5,
            activate: false,
            automatic_reports: false,
        };
        assert_eq!(retry_seconds(&p, 0), 300);
        assert_eq!(retry_seconds(&p, 1), 600);
        assert_eq!(retry_seconds(&p, u32::MAX), 19200);
        assert_eq!(
            retry_seconds(
                &BackgroundPolicy {
                    interval_minutes: 1440,
                    ..p
                },
                2
            ),
            86400
        );
    }
}
