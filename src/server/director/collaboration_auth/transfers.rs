//! Host-owned file transport. Queue checkpoints live in the meta store.
use super::*;
use psf_guard_director_meta::transfer::{Destination, FileDelivery, Job, State as TransferState};
use workflows::evidence::{self, Selection};
mod prepare;
mod worker;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    selection: Selection,
    #[serde(default)]
    output: Option<Output>,
    #[serde(default)]
    confirm_wbpp_membership: bool,
    #[serde(default)]
    automatic: bool,
    #[serde(default)]
    upload_pending: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    attempt: Option<Uuid>,
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    source_versions: std::collections::BTreeMap<Uuid, String>,
}

impl Request {
    fn review_required(&self, manifest: &Value) -> bool {
        !(self.automatic || self.upload_pending)
            || manifest["review_reasons"]
                .as_array()
                .is_some_and(|reasons| !reasons.is_empty())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Output {
    job_id: String,
    group_index: usize,
    revision: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Artifact {
    name: String,
    kind: String,
    path: PathBuf,
    bytes: u64,
    modified: u128,
    hash: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Package {
    staging_root: PathBuf,
    #[serde(default)]
    owner: Option<String>,
    catalog_id: Uuid,
    rig_id: Uuid,
    base_url: String,
    agent_id: String,
    project: String,
    label: String,
    catalog: String,
    manifest: Value,
    guard: String,
    artifacts: Vec<Artifact>,
    warnings: Vec<String>,
}

#[derive(Clone, Default, Serialize)]
pub(super) struct Progress {
    job_id: Option<Uuid>,
    file: String,
    bytes: u64,
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/transfers", axum::routing::get(activity))
}

async fn activity(State(state): State<Arc<AppState>>) -> Result<Json<ApiResponse<Value>>, Error> {
    let Some(service) = state.director.clone() else {
        return Ok(Json(ApiResponse::success(json!({"jobs":[]}))));
    };
    let jobs = jobs(&state).await?;
    let policies = service.clone().query(|s| s.transfer_policies()).await?;
    Ok(Json(ApiResponse::success(
        json!({"jobs":jobs,"policies":policies}),
    )))
}

/// The transfer jobs as the transfers route lists them, with the upload in
/// progress filled in. Empty when the server runs no Director service.
pub(in crate::server::director) async fn jobs(state: &Arc<AppState>) -> Result<Vec<Value>, Error> {
    let Some(service) = state.director.clone() else {
        return Ok(Vec::new());
    };
    let mut jobs = service.clone().query(|s| s.transfer_activity()).await?;
    let progress = service
        .collaboration
        .transfers
        .lock()
        .map_err(|_| Error::Internal)?
        .clone();
    for job in &mut jobs {
        if progress
            .job_id
            .is_some_and(|id| job["id"] == id.to_string())
            && job["state"] == "uploading"
        {
            job["current_file"] = json!(progress.file);
            job["sent_bytes"] = json!(progress.bytes);
        }
    }
    Ok(jobs)
}

fn view(job: &Job, progress: &Progress) -> Value {
    let package = job
        .package
        .as_ref()
        .and_then(|p| serde_json::from_value::<Package>(p.clone()).ok());
    let total = package
        .as_ref()
        .map_or(0, |p| p.artifacts.iter().map(|a| a.bytes).sum::<u64>());
    let done = package.as_ref().map_or(0, |p| {
        p.artifacts
            .iter()
            .filter(|a| job.completed.contains(&a.name))
            .map(|a| a.bytes)
            .sum::<u64>()
    });
    let active = progress.job_id == Some(job.id) && job.state == TransferState::Uploading;
    json!({"id":job.id,"connection_id":job.connection_id,"revision":job.revision,"state":job.state,
        "catalog":job.request["selection"]["catalog"],"label":package.as_ref().map(|p|p.label.as_str()).unwrap_or("Preparing contribution files"),
        "total_files":package.as_ref().map_or(0,|p|p.artifacts.len()),"completed_files":job.completed.len(),
        "total_bytes":total,"completed_bytes":done,"current_file":if active {progress.file.as_str()} else {""},
        "sent_bytes":if active {progress.bytes} else {0},"error":job.error,"next_attempt_ms":job.next_attempt_ms,
        "warnings":package.as_ref().map(|p|&p.warnings),"manifest":null,
        "automatic":psf_guard_director_meta::transfer::automatic(job)})
}

pub(super) async fn start(
    state: &Arc<AppState>,
    service: Arc<Service>,
    b: &ConnectionBinding,
    mut request: Request,
    upload_pending: bool,
) -> Result<Value, Failure> {
    if state.shutdown.is_stopping() {
        return Err(Failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "Server is stopping; try again after restart",
        ));
    }
    if b.state != ConnectionState::Registered {
        return Err(conflict());
    }
    let catalogs = background::catalogs(state, service.clone(), b.rig_id).await?;
    if catalogs.len() != 1 || catalogs[0].1.id != request.selection.catalog {
        return Err(conflict());
    }
    let connection = b.id;
    request.automatic = false;
    let selection = request.selection.clone();
    let review_required = service
        .clone()
        .query(move |s| {
            s.transfer_review_required(
                connection,
                selection.import_id,
                selection.panel,
                &selection.image_guids,
            )
        })
        .await?;
    request.upload_pending = upload_pending && !review_required;
    request.attempt = upload_pending.then(Uuid::new_v4);
    request.source_versions.clear();
    let value = serde_json::to_value(request).map_err(|_| invalid())?;
    let now = workflows::now_ms()?;
    let job = service
        .run(move |s| s.create_transfer(connection, value, now))
        .await?;
    Ok(json!({"transfer":view(&job,&Progress::default())}))
}

pub(super) async fn control(
    service: Arc<Service>,
    b: &ConnectionBinding,
    id: Uuid,
    revision: u64,
    action: String,
) -> Result<Value, Failure> {
    let connection = b.id;
    let job = service
        .run(move |s| {
            if s.transfer(id)?
                .is_none_or(|j| j.connection_id != connection)
            {
                return Err(StoreError::NotFound);
            }
            s.control_transfer(id, revision, &action)
        })
        .await?;
    Ok(json!({"transfer":view(&job,&Progress::default())}))
}

pub(super) async fn outputs(
    state: &Arc<AppState>,
    service: Arc<Service>,
    b: &ConnectionBinding,
    selection: Selection,
) -> Result<Value, Failure> {
    let (catalog, import) = prepare::context(state, service, b, &selection).await?;
    blocking(move || prepare::outputs(&catalog, &import, &selection)).await?
}

pub(super) async fn manifest_view(
    service: Arc<Service>,
    b: &ConnectionBinding,
    id: Uuid,
    revision: u64,
) -> Result<Value, Failure> {
    let connection = b.id;
    let job = service
        .query(move |s| s.transfer(id))
        .await?
        .ok_or_else(invalid)?;
    if job.connection_id != connection || job.revision != revision {
        return Err(conflict());
    }
    let package: Package =
        serde_json::from_value(job.package.ok_or_else(invalid)?).map_err(|_| invalid())?;
    Ok(json!({"transfer_manifest":package.manifest}))
}

pub(in crate::server::director) fn spawn(state: Arc<AppState>) {
    worker::spawn(state);
}

pub(super) async fn policy(
    state: &Arc<AppState>,
    service: Arc<Service>,
    b: &ConnectionBinding,
    automatic: bool,
    mode: String,
) -> Result<Value, Failure> {
    let catalogs = background::catalogs(state, service.clone(), b.rig_id).await?;
    if catalogs.len() != 1 {
        return Err(conflict());
    }
    let (connection, catalog) = (b.id, catalogs[0].0.id);
    service
        .run(move |s| s.set_transfer_policy(connection, catalog, automatic, &mode))
        .await?;
    Ok(json!({"saved":true}))
}
