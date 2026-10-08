//! Operator-owned work transfer. Remote work never grants equipment authority.
use super::*;
use psf_guard_director_interop::{
    collaboration,
    workflow::{self, Night, Settings},
};
mod evidence;
pub(super) fn now_ms() -> Result<u64, Failure> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .ok_or_else(invalid)
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
enum Input {
    BackgroundStatus {},
    BackgroundConfigure {
        expected: Option<psf_guard_director_meta::collaboration_connection::BackgroundPolicy>,
        policy: Option<psf_guard_director_meta::collaboration_connection::BackgroundPolicy>,
    },
    BackgroundRun {},
    ReportInputs {},
    ReportCandidates {
        import_id: Uuid,
        catalog: String,
        #[serde(default)]
        observing_night: Option<String>,
    },
    Configure {
        settings: Settings,
    },
    Browse {},
    Join {
        project: String,
        #[serde(default)]
        night: Option<Night>,
        #[serde(default)]
        observing_date: Option<String>,
    },
    Tonight {
        #[serde(default)]
        night: Option<Night>,
        #[serde(default)]
        observing_date: Option<String>,
    },
    Preview {
        task: String,
        night: Night,
    },
    Apply {
        task: String,
        night: Night,
        review_digest: String,
    },
    Checkin {},
    PreviewReport {
        selection: evidence::Selection,
    },
    QueueReport {
        selection: evidence::Selection,
        review_digest: String,
    },
}
pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/collaboration/{id}/work", axum::routing::post(action))
}
pub(super) fn forward_status(state: Arc<AppState>, rig: Uuid) {
    let Ok(service) = writable(&state) else {
        return;
    };
    tokio::spawn(async move {
        let Ok(_gate) = service.collaboration.gate.try_lock() else {
            return;
        };
        let Ok(connections) = service
            .clone()
            .query(move |s| s.collaboration_connections(rig))
            .await
        else {
            return;
        };
        for b in connections {
            if b.state != ConnectionState::Registered
                || !b.settings.as_ref().is_some_and(|s| s.share_status)
            {
                continue;
            }
            let should_send = if let Ok(mut last) = service.collaboration.last_delivery.lock() {
                let now = Instant::now();
                if last
                    .get(&b.id)
                    .is_some_and(|at| now.duration_since(*at) < Duration::from_secs(60))
                {
                    false
                } else {
                    last.insert(b.id, now);
                    true
                }
            } else {
                false
            };
            if should_send
                && execute(&state, service.clone(), b.id, Input::Checkin {})
                    .await
                    .is_err()
            {
                tracing::warn!(connection_id=%b.id,"Collaboration check-in failed; queued reports retained");
            }
        }
    });
}
async fn action(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Extension(access): Extension<RequestAccess>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> Response {
    let result = async {
        operator(&state, &access, &headers)?;
        let service = writable(&state)?;
        // Finish acknowledged imports/report receipts even if the browser leaves.
        tokio::spawn(async move {
            if matches!(input, Input::BackgroundStatus {}) {
                return execute(&state, service, id, input).await;
            }
            let _gate = service.collaboration.gate.lock().await;
            execute(&state, service.clone(), id, input).await
        })
        .await
        .map_err(|_| {
            Failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Collaboration operation interrupted; reload before retrying",
            )
        })?
    }
    .await;
    match result {
        Ok(value) => pairing::no_store(Json(ApiResponse::success(value))),
        Err(error) => error.into_response(),
    }
}
async fn execute(
    state: &Arc<AppState>,
    service: Arc<Service>,
    id: Uuid,
    input: Input,
) -> Result<Value, Failure> {
    let b = binding(service.clone(), id).await?;
    match &input {
        Input::BackgroundStatus {} => return background::status(state, service, &b).await,
        Input::BackgroundConfigure { expected, policy } => {
            return background::configure(state, service, b, expected.clone(), policy.clone()).await
        }
        Input::BackgroundRun {} => {
            background::run(state, service.clone(), b.clone()).await?;
            return background::status(state, service, &b).await;
        }
        Input::ReportInputs {} => return evidence::inputs(state, service, &b).await,
        Input::ReportCandidates {
            import_id,
            catalog,
            observing_night,
        } => {
            return evidence::candidates(
                state,
                service,
                &b,
                *import_id,
                catalog,
                observing_night.as_deref(),
            )
            .await
        }
        Input::PreviewReport { selection } => {
            return evidence::review(state, service, &b, selection.clone(), None).await
        }
        Input::QueueReport {
            selection,
            review_digest,
        } => {
            return evidence::review(
                state,
                service,
                &b,
                selection.clone(),
                Some(review_digest.clone()),
            )
            .await
        }
        _ => {}
    }
    if let Input::Configure { settings } = input {
        settings.validate().map_err(|_| invalid())?;
        let mut new = b.clone();
        new.settings = Some(settings);
        let copy = new.clone();
        service
            .run(move |s| s.update_collaboration_connection(&b, &copy))
            .await?;
        return Ok(json!({"binding":new}));
    }
    if b.state != ConnectionState::Registered || b.agent_id.is_none() {
        return Err(conflict());
    }
    let path = registry(state)?;
    let copy = b.clone();
    let token = file(move || credentials::read(&path, &copy))
        .await?
        .ok_or(Failure(
            StatusCode::CONFLICT,
            "Credential missing; original work and reports retained",
        ))?;
    let remote = Remote::new(&b)?;
    let source = b.source().map_err(Error::from)?;
    let input = match input {
        Input::Tonight {
            night,
            observing_date,
        } => Input::Tonight {
            night: Some(resolve_night(service.clone(), b.rig_id, night, observing_date).await?),
            observing_date: None,
        },
        Input::Join {
            project,
            night,
            observing_date,
        } => Input::Join {
            project,
            night: Some(resolve_night(service.clone(), b.rig_id, night, observing_date).await?),
            observing_date: None,
        },
        other => other,
    };
    let result = connected(service.clone(), &b, &source, &remote, &token, input).await;
    if matches!(&result, Err(Failure(StatusCode::UNAUTHORIZED, _))) {
        change(service, b, ConnectionState::Rejected, None).await?;
    }
    result
}

pub(super) async fn resolve_night(
    service: Arc<Service>,
    rig: Uuid,
    explicit: Option<Night>,
    date: Option<String>,
) -> Result<Night, Failure> {
    if let Some(night) = explicit {
        if date.is_some() {
            return Err(invalid());
        }
        night.validate().map_err(|_| invalid())?;
        return Ok(night);
    }
    let site = service
        .query(move |s| {
            let profile = s.rig_profile(rig)?;
            Ok(s.rig_site(rig, profile.as_ref())?.location)
        })
        .await?
        .ok_or(Failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Set this rig's observing site before pulling tonight's work",
        ))?;
    let now = now_ms()?;
    blocking(move || match date {
        Some(date) => Night::for_date(site, &date),
        None => Night::for_site(site, now),
    })
    .await?
    .map_err(|_| {
        Failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No astronomical observing night is available for this rig's site and date",
        )
    })
}
pub(super) async fn hello(
    service: Arc<Service>,
    b: &ConnectionBinding,
    remote: &Remote,
    token: &str,
) -> Result<(), Failure> {
    let settings = b.settings.as_ref().ok_or(Failure(
        StatusCode::CONFLICT,
        "Configure collaboration filters and exposures first",
    ))?;
    let rig = b.rig_id;
    let (profile, status) = service
        .query(move |s| Ok((s.rig_profile(rig)?, s.rig_status(rig)?)))
        .await?;
    let optics = profile
        .as_ref()
        .and_then(|p| p.optics.as_ref())
        .ok_or(Failure(
            StatusCode::CONFLICT,
            "Commission this rig's optics before requesting collaboration work",
        ))?;
    let now = now_ms()?;
    let presence = if settings.share_status {
        status.and_then(|s| current_presence(&s, now))
    } else {
        None
    };
    let body = settings
        .hello(&optics.value, &b.name, presence)
        .map_err(|_| invalid())?;
    workflow::hello_reply(
        &remote.post("agent/hello", Some(token), body).await?,
        &b.source().map_err(Error::from)?,
    )
    .map_err(|_| invalid())
}
fn current_presence(status: &psf_guard_director_meta::inbox::RigStatus, now: u64) -> Option<Value> {
    // Both receipt and rig-observation clocks must be fresh. Replayed telemetry
    // must not revive old activity, and names/coordinates stay private.
    if now.checked_sub(status.received_at_ms)? > collaboration::MAX_PRESENCE_AGE_MS {
        return None;
    }
    let state = status.payload["phase"].as_str()?.to_owned();
    collaboration::presence(
        &collaboration::LiveStatus {
            observed_at_ms: status.reported_at_ms,
            valid_for_ms: status.payload["fresh_for_ms"].as_u64().unwrap_or(30_000),
            state,
            target: None,
            project: None,
            telescope: None,
            ra_hours: None,
            dec_degrees: None,
        },
        now,
    )
    .ok()
    .flatten()
}
pub(super) async fn tonight(
    remote: &Remote,
    token: &str,
    night: &Night,
) -> Result<Vec<u8>, Failure> {
    let query = night.query().map_err(|_| invalid())?;
    // A named observing night is mandatory, never the server's rolling fallback.
    remote
        .send_query("agent/task", Some(token), None, &query)
        .await
}
async fn connected(
    service: Arc<Service>,
    b: &ConnectionBinding,
    source: &Source,
    remote: &Remote,
    token: &str,
    input: Input,
) -> Result<Value, Failure> {
    match input {
        Input::BackgroundStatus {}
        | Input::BackgroundConfigure { .. }
        | Input::BackgroundRun {}
        | Input::Configure { .. }
        | Input::PreviewReport { .. }
        | Input::QueueReport { .. }
        | Input::ReportInputs {}
        | Input::ReportCandidates { .. } => Err(invalid()),
        Input::Browse {} => {
            hello(service, b, remote, token).await?;
            let bytes = remote.get("agent/projects", Some(token), None).await?;
            Ok(json!(
                astrocollab::decode_projects(&bytes, source).map_err(|_| invalid())?
            ))
        }
        Input::Join { project, night, .. } => {
            if !workflow::valid_remote_id(&project) {
                return Err(invalid());
            }
            let night = night.ok_or_else(invalid)?;
            night.validate().map_err(|_| invalid())?;
            hello(service, b, remote, token).await?;
            let body = night
                .join(b.settings.as_ref().ok_or_else(invalid)?)
                .map_err(|_| invalid())?;
            // Join is idempotent. Validate the subsequent authenticated nightly
            // list, rather than treating the join acknowledgement as a plan.
            remote
                .post(&format!("agent/projects/{project}/join"), Some(token), body)
                .await?;
            let mut result = json!(astrocollab::decode_tonight(
                &tonight(remote, token, &night).await?,
                source,
                &night.night
            )
            .map_err(|_| invalid())?);
            result["night"] = json!(night);
            Ok(result)
        }
        Input::Tonight { night, .. } => {
            let night = night.ok_or_else(invalid)?;
            night.validate().map_err(|_| invalid())?;
            hello(service, b, remote, token).await?;
            let mut result = json!(astrocollab::decode_tonight(
                &tonight(remote, token, &night).await?,
                source,
                &night.night
            )
            .map_err(|_| invalid())?);
            result["night"] = json!(night);
            Ok(result)
        }
        Input::Preview { task, night }
        | Input::Apply {
            task,
            night,
            review_digest: _,
        } if !workflow::valid_remote_id(&task) || night.validate().is_err() => Err(invalid()),
        operation @ (Input::Preview { .. } | Input::Apply { .. }) => {
            let (task, night, review) = match operation {
                Input::Preview { task, night } => (task, night, None),
                Input::Apply {
                    task,
                    night,
                    review_digest,
                } => (task, night, Some(review_digest)),
                _ => unreachable!(),
            };
            hello(service.clone(), b, remote, token).await?;
            let plan = collaboration::prepare_import(
                &tonight(remote, token, &night).await?,
                source,
                &night.night,
                &task,
            )
            .map_err(|_| {
                Failure(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "This share needs review and cannot be imported as executable work",
                )
            })?;
            let rig = b.rig_id;
            service
                .run(move |s| {
                    let preview = s.preview_collaboration_import(&plan, rig)?;
                    if let Some(review) = review {
                        s.apply_collaboration_import(
                            &plan,
                            rig,
                            &review,
                            now_ms().map_err(|_| StoreError::InvalidInput)?,
                        )?;
                    }
                    Ok(json!({"preview":preview,"plan":plan}))
                })
                .await
                .map_err(Into::into)
        }
        Input::Checkin {} => {
            hello(service.clone(), b, remote, token).await?;
            let key = source.clone();
            let reports = service
                .clone()
                .query(move |s| s.pending_collaboration_reports(&key, collaboration::MAX_REPORTS))
                .await?;
            if reports.is_empty() {
                return Ok(json!({"delivered":0,"accepted":0,"rejected":0}));
            }
            let ids = reports.iter().map(|r| r.id).collect::<Vec<_>>();
            let reply = remote
                .post(
                    "agent/report",
                    Some(token),
                    json!({"contributions":reports.iter().map(|r|&r.payload).collect::<Vec<_>>()}),
                )
                .await?;
            let key = source.clone();
            let now = now_ms()?;
            let received = service
                .run(move |s| s.acknowledge_collaboration_reports(&key, &ids, &reply, now))
                .await?;
            let accepted = received
                .iter()
                .filter(|r| r.recorded.as_ref().is_some_and(|v| v.accepted))
                .count();
            Ok(
                json!({"delivered":received.len(),"accepted":accepted,"rejected":received.len()-accepted,"receipts":received}),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn receipt_freshness_does_not_revive_replayed_presence() {
        let mut s = psf_guard_director_meta::inbox::RigStatus {
            rig_id: Uuid::new_v4(),
            session_id: "session".into(),
            reported_at_ms: 1_000,
            received_at_ms: 100_000,
            payload: json!({"phase":"imaging","fresh_for_ms":60_000,"target":"private"}),
        };
        assert!(current_presence(&s, 100_001).is_none());
        s.reported_at_ms = 100_000;
        let presence = current_presence(&s, 100_001).unwrap();
        assert_eq!(presence["state"], "imaging");
        assert!(presence.get("target").is_none());
        assert!(presence["ra"].is_null());
    }
}
