//! Tell a coordinator that this server left one of its projects: decline
//! each task the plan's imports came from, with the rig's saved sign-in.
//! A declined task leaves the rig's list at the coordinator and gets no
//! more panels. See docs/design/collaboration.md, "Leaving a collaboration".
use super::*;
use psf_guard_director_meta::collaboration_leave::RemoteTask;

/// What telling the coordinator did.
#[derive(Clone, Debug, Default, Serialize)]
pub(in crate::server::director) struct Told {
    /// The collaboration server's host.
    pub host: String,
    /// Tasks the coordinator declined.
    pub declined: Vec<String>,
    /// Why a task could not be declined, one sentence each.
    pub problems: Vec<String>,
}

/// The same check as joining a project: an interactive editor's same-origin
/// request on a server with database management. The message explains a
/// refusal.
pub(in crate::server::director) fn interactive(
    state: &AppState,
    access: &RequestAccess,
    headers: &HeaderMap,
) -> Result<(), &'static str> {
    operator(state, access, headers)
        .map(|_| ())
        .map_err(|f| f.1)
}

/// The host of a collaboration server's URL, as the dialogs show it.
pub(in crate::server::director) fn host(base_url: &str) -> String {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            Some(match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host,
            })
        })
        .unwrap_or_else(|| base_url.to_owned())
}

/// Every connection to one collaboration server.
async fn connections(
    service: Arc<Service>,
    base_url: String,
) -> Result<Vec<ConnectionBinding>, Failure> {
    service
        .query(move |s| {
            let mut found = Vec::new();
            for id in s.collaboration_connection_ids()? {
                if let Some(b) = s.collaboration_connection(id)?
                    && b.base_url == base_url
                {
                    found.push(b);
                }
            }
            Ok(found)
        })
        .await
        .map_err(Into::into)
}

/// The registered connection for one agent at the server, with its saved
/// token, or why there is none.
async fn signed_in(
    state: &AppState,
    connections: &[ConnectionBinding],
    agent: &str,
) -> Result<(ConnectionBinding, String), String> {
    let b = connections
        .iter()
        .find(|b| b.agent_id.as_deref() == Some(agent) && b.state == ConnectionState::Registered)
        .cloned()
        .ok_or_else(|| {
            format!("No rig on this server is signed in as agent {agent} at this collaboration.")
        })?;
    let path = registry(state).map_err(|f| f.1.to_owned())?;
    let copy = b.clone();
    match file(move || credentials::read(&path, &copy)).await {
        Ok(Some(token)) => Ok((b, token)),
        Ok(None) => Err(format!("The saved sign-in for {} is missing.", b.name)),
        Err(f) => Err(f.1.to_owned()),
    }
}

/// Whether each task's rig has a registered connection with a saved
/// sign-in, so leaving can decline the tasks at the coordinator.
pub(in crate::server::director) async fn reachable(
    state: &AppState,
    service: Arc<Service>,
    base_url: String,
    tasks: &[RemoteTask],
) -> bool {
    let Ok(connections) = connections(service, base_url).await else {
        return false;
    };
    for task in tasks {
        if signed_in(state, &connections, &task.agent_id)
            .await
            .is_err()
        {
            return false;
        }
    }
    true
}

/// Decline each task at the coordinator. A rejected sign-in marks its
/// connection for repair, as every other request does. Nothing local
/// depends on the answer: the plan has left the collaboration already.
pub(in crate::server::director) async fn decline(
    state: &AppState,
    service: Arc<Service>,
    base_url: &str,
    tasks: &[RemoteTask],
) -> Told {
    let host = host(base_url);
    let mut told = Told {
        host: host.clone(),
        ..Told::default()
    };
    let connections = match connections(service.clone(), base_url.to_owned()).await {
        Ok(found) => found,
        Err(failure) => {
            told.problems.push(failure.1.to_owned());
            return told;
        }
    };
    for task in tasks {
        let (b, token) = match signed_in(state, &connections, &task.agent_id).await {
            Ok(found) => found,
            Err(problem) => {
                told.problems.push(problem);
                continue;
            }
        };
        let sent = async {
            Remote::new(&b)?
                .post(
                    &format!("agent/task/{}", task.task_id),
                    Some(&token),
                    json!({"state": "declined"}),
                )
                .await
        }
        .await;
        match sent {
            Ok(_) => told.declined.push(task.task_id.clone()),
            Err(Failure(StatusCode::UNAUTHORIZED, _)) => {
                if let Err(error) =
                    change(service.clone(), b.clone(), ConnectionState::Rejected, None).await
                {
                    tracing::warn!(connection_id = %b.id, reason = error.1, "Could not mark a rejected collaboration sign-in");
                }
                told.problems.push(format!(
                    "{host} rejected the saved sign-in for {}. Reconnect it in the rig's Collaboration settings.",
                    b.name
                ));
            }
            Err(failure) => told.problems.push(format!(
                "Task {} was not declined at {host}: {}",
                task.task_id, failure.1
            )),
        }
    }
    told
}

/// Forget the background runs of every connection to the server, as saving
/// a policy does: the next sweep starts from the policy and imports as they
/// are now, not from a report scan position inside an import that is gone.
pub(in crate::server::director) async fn forget_runs(service: Arc<Service>, base_url: String) {
    let Ok(connections) = connections(service.clone(), base_url).await else {
        return;
    };
    if let Ok(mut records) = service.collaboration.background.lock() {
        for b in connections {
            records.remove(&b.id);
        }
    }
}

/// Hold the collaboration gate, which nightly intake, automatic reports and
/// every interactive collaboration operation take. Leaving and deleting a
/// plan take it before the rig-database turn and the sync lock, the order
/// nightly intake takes them in when it activates work.
pub(in crate::server::director) async fn hold(
    service: &Service,
) -> Result<tokio::sync::MutexGuard<'_, ()>, Error> {
    tokio::time::timeout(ADMISSION_WAIT, service.collaboration.gate.lock())
        .await
        .map_err(|_| Error::Busy)
}
