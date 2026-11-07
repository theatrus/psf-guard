//! A rig's Director plugin in one view, from pairing to the work it holds, so
//! the operator can see what is left to do and why the rig gets no work.
use super::*;
use crate::server::auth::RequestAccess;
use axum::Extension;
use psf_guard_director_core::program::Configuration;
use psf_guard_director_meta::{
    client::Client,
    equipment_report::{EquipmentReport, MAX_REPORT_AGE_MS},
    profile::CameraDefaults,
    workload::Policy,
};
use std::collections::BTreeMap;

#[derive(Serialize)]
pub(super) struct PluginView {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    rig_id: Uuid,
    /// Zero before the rig has a saved profile.
    profile_revision: u64,
    /// The equipment an operator accepted.
    configuration: Option<Configuration>,
    filter_names: BTreeMap<String, String>,
    camera_defaults: Option<CameraDefaults>,
    clients: Vec<PluginClient>,
    /// Plans activated on this rig, which allowed work runs.
    plans: Vec<ActivatedPlan>,
    policy: Option<Policy>,
    work: WorkState,
    /// What the rig's program leaves out, and why. Empty until work is allowed.
    left_out: Vec<String>,
    grant: Option<Grant>,
    /// The plugin's last call of any kind.
    last_contact_at_ms: Option<u64>,
}

#[derive(Serialize)]
struct PluginClient {
    #[serde(flatten)]
    client: Client,
    report: Option<EquipmentReport>,
    review: Review,
}

/// What an operator can do with a client's equipment report.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum Review {
    /// The plugin has not reported.
    None,
    Ready,
    /// Too old to accept: report again from the plugin.
    Old,
    /// Open work keeps the rig's equipment as it is.
    WorkOpen,
    Accepted,
}

#[derive(Serialize)]
struct ActivatedPlan {
    id: Uuid,
    name: String,
}

/// Whether the plugin can get work, and if not, why.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
enum WorkState {
    NotAllowed,
    Off,
    Ready,
    /// The client the policy allows no longer exists.
    PluginRemoved,
    /// The accepted equipment is no longer what the policy allows.
    EquipmentChanged,
    /// The rig's setup changed since the policy was saved.
    SetupChanged,
    /// A plan is activated on the rig outside the policy's list of plans.
    PlansChanged,
    /// Allowed, but no activated plan forms a program for this rig.
    NothingToRun,
}

#[derive(Serialize)]
struct Grant {
    allocation_id: Uuid,
    client_id: Uuid,
    admitted_at_ms: u64,
    expires_at_ms: Option<u64>,
    started: bool,
    /// The rig takes no new work while this is true.
    open: bool,
    /// False once the client holding the work is removed: only then can an
    /// operator close it.
    plugin_paired: bool,
    closed_at_ms: Option<u64>,
}

fn review(report: Option<&EquipmentReport>, work_open: bool, now: u64) -> Review {
    let Some(report) = report else {
        return Review::None;
    };
    if report.accepted_revision.is_some() {
        Review::Accepted
    } else if now.saturating_sub(report.observed_at_ms.min(report.received_at_ms))
        > MAX_REPORT_AGE_MS
    {
        Review::Old
    } else if work_open {
        Review::WorkOpen
    } else {
        Review::Ready
    }
}

fn work_state(
    policy: Option<&Policy>,
    clients: &[Client],
    revision: u64,
    configuration: Option<&Configuration>,
    plans: &[ActivatedPlan],
) -> WorkState {
    let Some(policy) = policy else {
        return WorkState::NotAllowed;
    };
    if !policy.enabled {
        WorkState::Off
    } else if !clients.iter().any(|c| c.client_id == policy.client_id) {
        WorkState::PluginRemoved
    } else if configuration.is_none_or(|c| c.id != policy.configuration_id) {
        WorkState::EquipmentChanged
    } else if revision != policy.profile_revision {
        WorkState::SetupChanged
    } else if !policy.project_ids.is_empty()
        && plans.iter().any(|p| !policy.project_ids.contains(&p.id))
    {
        WorkState::PlansChanged
    } else {
        WorkState::Ready
    }
}

/// Check an allowed rig against the program the plugin would pull, and list
/// what that program leaves out.
fn program_state(
    gathered: Result<super::program::Gathered, super::program::PullError>,
) -> (WorkState, Vec<String>) {
    match gathered {
        Ok(gathered) if gathered.goals().is_empty() => {
            (WorkState::NothingToRun, gathered.omitted().to_vec())
        }
        Ok(gathered) => (WorkState::Ready, gathered.omitted().to_vec()),
        Err(super::program::PullError::NotReady(reason)) => (WorkState::NothingToRun, vec![reason]),
        Err(_) => (WorkState::Ready, Vec::new()),
    }
}

pub(super) async fn get(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
    Extension(access): Extension<RequestAccess>,
) -> Response {
    if let Err(response) = pairing::operator(&access) {
        return *response;
    }
    let answer = async {
        let service = enabled(&state)?;
        let catalog = state.get_database(&slug).ok_or(Error::Missing)?;
        let (identity, _) = rig_profile::file_identity(&service, catalog).await?;
        let instance = service.instance_id;
        let catalogs: Vec<_> = state
            .databases
            .read()
            .map_err(|_| Error::Internal)?
            .values()
            .cloned()
            .collect();
        let now = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| Error::Internal)?
                .as_millis(),
        )
        .map_err(|_| Error::Internal)?;
        service
            .with_reader(move |store| {
                let binding = store.catalog_rig(identity.id)?.ok_or(Error::Missing)?;
                let rig = binding.rig.id;
                let clients = store.clients(rig)?;
                let reports = store.equipment_reports(rig)?;
                let profile = store.rig_profile(rig)?;
                let policy = store.workload_policy(rig)?;
                let grant = match store.allocation(rig)? {
                    Some(grant) => {
                        let recorded = store.workload(rig, grant.client_id, grant.allocation_id)?;
                        Some(Grant {
                            allocation_id: grant.allocation_id,
                            client_id: grant.client_id,
                            admitted_at_ms: grant.admitted_at_ms,
                            expires_at_ms: grant.snapshot["program"]["assignment"]["expires_at_ms"]
                                .as_u64(),
                            started: store.allocation_started(rig, grant.allocation_id)?,
                            open: !recorded.as_ref().is_some_and(|w| w.released),
                            plugin_paired: clients.iter().any(|c| c.client_id == grant.client_id),
                            closed_at_ms: recorded.and_then(|w| w.closed_at_ms),
                        })
                    }
                    None => None,
                };
                let work_open = grant.as_ref().is_some_and(|g| g.open);
                let (revision, configuration, filter_names, camera_defaults) = profile.map_or_else(
                    || (0, None, BTreeMap::new(), None),
                    |p| {
                        (
                            p.revision,
                            p.configuration.map(|c| c.value),
                            p.filter_names,
                            p.camera_defaults,
                        )
                    },
                );
                let mut plans = Vec::new();
                for activation in store.activations_for_rig(rig)? {
                    if let Some(project) = store.project(activation.project_id)? {
                        plans.push(ActivatedPlan {
                            id: project.id,
                            name: project.name,
                        });
                    }
                }
                plans.sort_by(|a, b| a.name.cmp(&b.name));
                let work = work_state(
                    policy.as_ref(),
                    &clients,
                    revision,
                    configuration.as_ref(),
                    &plans,
                );
                let (work, left_out) = if work == WorkState::Ready && !plans.is_empty() {
                    program_state(super::program::gather(
                        store,
                        &catalogs,
                        instance,
                        rig,
                        identity.id,
                        false,
                    ))
                } else {
                    (work, Vec::new())
                };
                let clients = clients
                    .into_iter()
                    .map(|client| {
                        let report = reports
                            .iter()
                            .find(|r| r.client_id == client.client_id)
                            .cloned();
                        let review = review(report.as_ref(), work_open, now);
                        PluginClient {
                            client,
                            report,
                            review,
                        }
                    })
                    .collect();
                let last_contact_at_ms = store
                    .contacts_for_rig(rig)?
                    .into_iter()
                    .map(|contact| contact.at_ms)
                    .max();
                Ok::<_, Error>(PluginView {
                    coordinator_instance_id: instance,
                    catalog_id: identity.id,
                    rig_id: rig,
                    profile_revision: revision,
                    configuration,
                    filter_names,
                    camera_defaults,
                    clients,
                    plans,
                    policy,
                    work,
                    left_out,
                    grant,
                    last_contact_at_ms,
                })
            })
            .await
    }
    .await;
    match answer {
        Ok(view) => pairing::no_store(Json(ApiResponse::success(view))),
        Err(error) => pairing::no_store(error.into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_that_cannot_form_is_not_ready_work() {
        use super::super::program::PullError;
        let (state, left_out) = program_state(Err(PullError::NotReady("no plan".into())));
        assert_eq!(state, WorkState::NothingToRun);
        assert_eq!(left_out, ["no plan"]);
        // A failed read says nothing about the plans.
        let (state, left_out) = program_state(Err(PullError::Api(Error::Internal)));
        assert_eq!(state, WorkState::Ready);
        assert!(left_out.is_empty());
    }

    fn policy(client: Uuid) -> Policy {
        Policy {
            rig_id: Uuid::new_v4(),
            catalog_id: Uuid::new_v4(),
            client_id: client,
            profile_id: Uuid::new_v4(),
            profile_revision: 3,
            configuration_id: "config-1".into(),
            project_ids: vec![Uuid::new_v4()],
            enabled: true,
            revision: 1,
        }
    }

    fn client(id: Uuid) -> Client {
        Client {
            client_id: id,
            catalog_id: Uuid::new_v4(),
            rig_id: Uuid::new_v4(),
            profile_id: Uuid::new_v4(),
            client_name: "NINA".into(),
            created_at_ms: 1,
        }
    }

    fn configuration(id: &str) -> Configuration {
        let mut value: psf_guard_director_core::program::Program = serde_json::from_str(
            include_str!("../../../crates/director-core/tests/fixtures/execution-program.json"),
        )
        .unwrap();
        value.configuration.id = id.into();
        value.configuration
    }

    #[test]
    fn work_state_gives_the_reason_a_policy_no_longer_applies() {
        let id = Uuid::new_v4();
        let clients = [client(id)];
        let accepted = configuration("config-1");
        let other = configuration("config-2");
        let allowed = policy(id);
        let state = |p, c, r, a| work_state(p, c, r, a, &[]);
        assert_eq!(
            state(None, &clients, 3, Some(&accepted)),
            WorkState::NotAllowed
        );
        assert_eq!(
            state(Some(&allowed), &clients, 3, Some(&accepted)),
            WorkState::Ready
        );
        assert_eq!(
            state(Some(&allowed), &clients, 4, Some(&accepted)),
            WorkState::SetupChanged
        );
        assert_eq!(
            state(Some(&allowed), &[], 3, Some(&accepted)),
            WorkState::PluginRemoved
        );
        assert_eq!(
            state(Some(&allowed), &clients, 4, Some(&other)),
            WorkState::EquipmentChanged
        );
        let off = Policy {
            enabled: false,
            ..allowed.clone()
        };
        assert_eq!(state(Some(&off), &[], 9, None), WorkState::Off);
    }

    #[test]
    fn a_plan_outside_a_list_stops_work_but_an_empty_list_takes_every_plan() {
        let id = Uuid::new_v4();
        let clients = [client(id)];
        let accepted = configuration("config-1");
        let plans = [ActivatedPlan {
            id: Uuid::new_v4(),
            name: "Activated later".into(),
        }];
        let listed = policy(id);
        assert_eq!(
            work_state(Some(&listed), &clients, 3, Some(&accepted), &plans),
            WorkState::PlansChanged
        );
        let every = Policy {
            project_ids: vec![],
            ..listed
        };
        assert_eq!(
            work_state(Some(&every), &clients, 3, Some(&accepted), &plans),
            WorkState::Ready
        );
    }
}
