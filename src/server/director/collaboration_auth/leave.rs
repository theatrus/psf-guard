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
    /// Tasks there were to decline.
    pub tasks: usize,
    /// Tasks the coordinator declined.
    pub declined: Vec<String>,
    /// Why a task could not be declined, one sentence each.
    pub problems: Vec<String>,
}

/// What a person is doing when PSF Guard signs in to the coordinator for
/// them, for the refusal message.
#[derive(Clone, Copy, Debug)]
pub(in crate::server::director) enum Purpose {
    Leave,
    DeletePlan,
}

/// The same check as joining a project: an interactive editor's same-origin
/// request on a server with database management. The message explains a
/// refusal in terms of what the person was doing.
pub(in crate::server::director) fn interactive(
    state: &AppState,
    access: &RequestAccess,
    headers: &HeaderMap,
    purpose: Purpose,
) -> Result<(), &'static str> {
    use Purpose::*;
    check_operator(state, access, headers)
        .map(|_| ())
        .map_err(|refusal| match (refusal, purpose) {
            (NotOperator::Editor, Leave) => {
                "Leaving a collaboration needs an editor signed in to PSF Guard, not an API token, on a server with database management."
            }
            (NotOperator::Editor, DeletePlan) => {
                "Deleting a plan from a collaboration needs an editor signed in to PSF Guard, not an API token, on a server with database management."
            }
            (NotOperator::BadOrigin | NotOperator::CrossOrigin, Leave) => {
                "Leave the collaboration from PSF Guard's own page; requests from another site are refused."
            }
            (NotOperator::BadOrigin | NotOperator::CrossOrigin, DeletePlan) => {
                "Delete the plan from PSF Guard's own page; requests from another site are refused."
            }
            (NotOperator::SignIn, Leave) => "Sign in again, then leave the collaboration.",
            (NotOperator::SignIn, DeletePlan) => "Sign in again, then delete the plan.",
        })
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

/// The registered connection for one task's agent at the server, with its
/// saved token, or why there is none.
async fn signed_in(
    state: &AppState,
    connections: &[ConnectionBinding],
    task: &RemoteTask,
) -> Result<(ConnectionBinding, String), String> {
    let rig = &task.rig;
    let b = connections
        .iter()
        .find(|b| {
            b.agent_id.as_deref() == Some(task.agent_id.as_str())
                && b.state == ConnectionState::Registered
        })
        .cloned()
        .ok_or_else(|| format!("{rig} is not signed in to this collaboration."))?;
    let path = registry(state).map_err(|f| f.1.to_owned())?;
    let copy = b.clone();
    match file(move || credentials::read(&path, &copy)).await {
        Ok(Some(token)) => Ok((b, token)),
        Ok(None) => Err(format!("The saved sign-in for {rig} is missing.")),
        Err(f) => Err(f.1.to_owned()),
    }
}

/// Which of the tasks' rigs can decline at the coordinator, and which
/// cannot, each named once.
#[derive(Clone, Debug, Default, Serialize)]
pub(in crate::server::director) struct Reach {
    pub signed_in: Vec<String>,
    pub not_signed_in: Vec<String>,
}

pub(in crate::server::director) async fn reach(
    state: &AppState,
    service: Arc<Service>,
    base_url: String,
    tasks: &[RemoteTask],
) -> Reach {
    let mut reach = Reach::default();
    let connections = connections(service, base_url).await.unwrap_or_default();
    for task in tasks {
        let list = if signed_in(state, &connections, task).await.is_ok() {
            &mut reach.signed_in
        } else {
            &mut reach.not_signed_in
        };
        if !list.contains(&task.rig) {
            list.push(task.rig.clone());
        }
    }
    reach
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
        tasks: tasks.len(),
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
        let (b, token) = match signed_in(state, &connections, task).await {
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
                    task.rig
                ));
            }
            Err(failure) => told.problems.push(format!(
                "{host} did not decline the task for {}: {}",
                task.rig, failure.1
            )),
        }
        if told.declined.last() != Some(&task.task_id) {
            tracing::warn!(
                %host,
                task = %task.task_id,
                rig = %task.rig,
                problem = told.problems.last().map(String::as_str).unwrap_or(""),
                "Task still to decline at the coordinator"
            );
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
