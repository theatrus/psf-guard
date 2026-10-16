use super::*;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, ReadBuf};

pub(super) fn spawn(state: Arc<AppState>) {
    let Some(service) = state.director.clone() else {
        return;
    };
    let weak = Arc::downgrade(&state);
    let shutdown = state.shutdown.clone();
    tokio::spawn(async move {
        if let Err(error) = service.clone().run(|s| s.recover_transfers()).await {
            tracing::error!(?error, "Could not recover collaboration transfers");
            return;
        }
        let mut next_scan = Instant::now();
        while !shutdown.is_stopping() {
            let Some(state) = weak.upgrade() else {
                break;
            };
            if state.database_management_allowed() {
                let work = shutdown.hold("collaboration file transfer");
                if Instant::now() >= next_scan {
                    if let Err(error) = automatic(&state, service.clone()).await {
                        tracing::warn!(reason = error.1, "Automatic transfer scan deferred");
                    }
                    next_scan = Instant::now() + Duration::from_secs(30);
                }
                if let Err(error) = step(&state, service.clone()).await {
                    tracing::warn!(
                        reason = error.1,
                        "Collaboration transfer worker retained its queue"
                    );
                }
                drop(work);
            }
            if !shutdown.sleep(Duration::from_secs(2)).await {
                break;
            }
        }
    });
}

async fn save(service: Arc<Service>, job: Job) -> Result<Job, Failure> {
    service
        .run(move |s| s.save_transfer(job))
        .await
        .map_err(Into::into)
}

async fn step(state: &Arc<AppState>, service: Arc<Service>) -> Result<(), Failure> {
    let now = workflows::now_ms()?;
    let Some(mut job) = service.clone().query(move |s| s.next_transfer(now)).await? else {
        return Ok(());
    };
    let outcome = run(state, service.clone(), &mut job).await;
    if let Err(error) = outcome {
        let retry = error.0.is_server_error() || error.0 == StatusCode::TOO_MANY_REQUESTS;
        job.state = if state.shutdown.is_stopping() {
            if job.package.is_some() {
                TransferState::Queued
            } else {
                TransferState::Preparing
            }
        } else if retry {
            TransferState::Retry
        } else {
            TransferState::NeedsReview
        };
        job.attempts = job.attempts.saturating_add(1);
        job.error = Some(error.1.into());
        job.next_attempt_ms = now
            .saturating_add((30_000u64.saturating_mul(1u64 << job.attempts.min(7))).min(3_600_000));
        // A user may have paused or cancelled while the HTTP request failed.
        let _ = save(service.clone(), job.clone()).await;
        tracing::warn!(transfer_id=%job.id,reason=error.1,"Collaboration transfer not complete");
    }
    *service
        .collaboration
        .transfers
        .lock()
        .map_err(|_| invalid())? = Progress::default();
    Ok(())
}

async fn run(state: &Arc<AppState>, service: Arc<Service>, job: &mut Job) -> Result<(), Failure> {
    let b = binding(service.clone(), job.connection_id).await?;
    if b.state != ConnectionState::Registered {
        return Err(Failure(
            StatusCode::CONFLICT,
            "Connection is not registered; repair it before resuming uploads",
        ));
    }
    let request: Request = serde_json::from_value(job.request.clone()).map_err(|_| invalid())?;
    let (catalog, import) =
        prepare::context(state, service.clone(), &b, &request.selection).await?;
    let path = catalog.database_path.clone();
    let instance = service.instance_id;
    let identity = blocking(move || identity_of(instance, &path)).await?;
    if job.package.is_none() {
        let (copy, plan, request, rig) =
            (catalog.clone(), import.clone(), request.clone(), b.rig_id);
        let automatic = request.automatic;
        let shutdown = state.shutdown.clone();
        let package = blocking(move || {
            prepare::build(&copy, &plan, &request, identity.id, rig, || {
                shutdown.is_stopping()
            })
        })
        .await??;
        job.completed
            .retain(|name| package.artifacts.iter().any(|a| &a.name == name));
        job.package = Some(serde_json::to_value(package).map_err(|_| invalid())?);
        job.state = if automatic {
            TransferState::Queued
        } else {
            TransferState::Review
        };
        job.error = None;
        *job = save(service, job.clone()).await?;
        return Ok(());
    }
    let mut package: Package =
        serde_json::from_value(job.package.clone().ok_or_else(invalid)?).map_err(|_| invalid())?;
    if package.rig_id != b.rig_id
        || package.catalog_id != identity.id
        || package.base_url != b.base_url
        || b.agent_id.as_deref() != Some(&package.agent_id)
    {
        return Err(conflict());
    }
    validate(&catalog, &import, &request, &package).await?;
    let credential_path = registry(state)?;
    let copy = b.clone();
    let token = file(move || credentials::read(&credential_path, &copy))
        .await?
        .ok_or(Failure(
            StatusCode::CONFLICT,
            "Upload credential is missing; repair the existing connection",
        ))?;
    let remote = Remote::new(&b)?;
    let files = tokio::select! {
        result = remote.get("files/projects", Some(&token), None) => result?,
        _ = state.shutdown.begun() => return Err(Failure(StatusCode::SERVICE_UNAVAILABLE, "Transfer stopped for application shutdown")),
    };
    let files: Value = serde_json::from_slice(&files).map_err(|_| invalid())?;
    if files["signedIn"] != true
        || !files["projects"].as_array().is_some_and(|projects| {
            projects
                .iter()
                .any(|p| p["id"] == package.project && p["collecting"] == true)
        })
    {
        return Err(Failure(
            StatusCode::CONFLICT,
            "This collaboration is not collecting files, or the telescope has no Discord owner",
        ));
    }
    let max_bytes = files["maxBytes"].as_u64().ok_or_else(invalid)?;
    let route = format!("projects/{}/files", package.project);
    let listing = tokio::select! {
        result = remote.get(&route, Some(&token), None) => result?,
        _ = state.shutdown.begun() => return Err(Failure(StatusCode::SERVICE_UNAVAILABLE, "Transfer stopped for application shutdown")),
    };
    let listing: Value = serde_json::from_slice(&listing).map_err(|_| invalid())?;
    let owner = listing["me"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(invalid)?
        .to_owned();
    if package
        .owner
        .as_ref()
        .is_some_and(|previous| previous != &owner)
    {
        return Err(Failure(
            StatusCode::CONFLICT,
            "The remote file owner changed; review the destination before retrying",
        ));
    }
    package.owner = Some(owner);
    job.package = Some(serde_json::to_value(&package).map_err(|_| invalid())?);
    if package.artifacts.iter().any(|a| a.bytes > max_bytes) {
        return Err(Failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "A package file exceeds the server's upload limit",
        ));
    }
    job.state = TransferState::Uploading;
    job.error = None;
    *job = save(service.clone(), job.clone()).await?;
    let received = listing["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| f["user"].as_str() == package.owner.as_deref())
        .filter_map(|f| {
            Some((
                f["name"].as_str()?.to_owned(),
                f["kind"].as_str()?.to_owned(),
                f["bytes"].as_u64()?,
            ))
        })
        .collect::<std::collections::BTreeSet<_>>();
    let completed = job
        .completed
        .iter()
        .cloned()
        .collect::<std::collections::BTreeSet<_>>();
    for artifact in &package.artifacts {
        if completed.contains(&artifact.name) {
            continue;
        }
        if state.shutdown.is_stopping() {
            return Err(Failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "Transfer stopped for application shutdown",
            ));
        }
        let current = service
            .clone()
            .query({
                let id = job.id;
                move |s| s.transfer_revision(id)
            })
            .await?
            .ok_or_else(invalid)?;
        if current != job.revision {
            return Ok(());
        }
        let already =
            received.contains(&(artifact.name.clone(), artifact.kind.clone(), artifact.bytes));
        if !already && !upload(state, service.clone(), job, &b, &package, artifact, &token).await? {
            return Ok(());
        }
        let (id, revision, name, bytes) =
            (job.id, job.revision, artifact.name.clone(), artifact.bytes);
        job.revision = service
            .clone()
            .run(move |s| s.acknowledge_transfer_file(id, revision, &name, bytes))
            .await?;
        job.completed.push(artifact.name.clone());
        *service
            .collaboration
            .transfers
            .lock()
            .map_err(|_| invalid())? = Progress::default();
        tracing::info!(transfer_id=%job.id,file=%artifact.name,bytes=artifact.bytes,already,completed=job.completed.len(),total=package.artifacts.len(),"Collaboration file acknowledged");
    }
    validate(&catalog, &import, &request, &package).await?;
    job.state = TransferState::Complete;
    *job = save(service, job.clone()).await?;
    Ok(())
}

async fn automatic(state: &Arc<AppState>, service: Arc<Service>) -> Result<(), Failure> {
    let mut policies = service.clone().query(|s| s.transfer_policies()).await?;
    if !policies.is_empty() {
        let offset = (workflows::now_ms()? / 30_000) as usize % policies.len();
        policies.rotate_left(offset);
    }
    let mut remaining = 8usize;
    for policy in policies.into_iter().filter(|p| p["automatic"] == true) {
        if remaining == 0 || state.shutdown.is_stopping() {
            break;
        }
        let id = policy["connection_id"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(invalid)?;
        let b = binding(service.clone(), id).await?;
        if b.state != ConnectionState::Registered {
            continue;
        }
        let catalogs = background::catalogs(state, service.clone(), b.rig_id).await?;
        if catalogs.len() != 1
            || catalogs[0].0.id.to_string() != policy["catalog_id"].as_str().unwrap_or("")
        {
            continue;
        }
        let rig = b.rig_id;
        let (imports, longitude) = service
            .clone()
            .query(move |s| {
                let profile = s.rig_profile(rig)?;
                Ok((
                    s.collaboration_imports_for_connection(id)?,
                    s.rig_site(rig, profile.as_ref())?
                        .location
                        .ok_or(StoreError::InvalidInput)?
                        .longitude_degrees,
                ))
            })
            .await?;
        let mut imports = imports;
        if !imports.is_empty() {
            let offset = (workflows::now_ms()? / 30_000) as usize % imports.len();
            imports.rotate_left(offset);
        }
        let started = Instant::now();
        for import in imports.into_iter().take(32) {
            if started.elapsed() > Duration::from_secs(15) {
                break;
            }
            let catalog = catalogs[0].1.clone();
            let import_id = import.plan.import_id();
            let selections = match blocking(move || {
                evidence::transfer_selections(&catalog, import_id, longitude)
            })
            .await?
            {
                Ok(selections) => selections,
                Err(error) => {
                    tracing::warn!(%import_id,reason=error.1,"Automatic transfer cohort held");
                    continue;
                }
            };
            for (_, selection) in selections {
                if remaining == 0 {
                    break;
                }
                let output = if policy["mode"] == "psf_guard" {
                    let (catalog, plan) = match prepare::context(
                        state,
                        service.clone(),
                        &b,
                        &selection,
                    )
                    .await
                    {
                        Ok(context) => context,
                        Err(error) => {
                            tracing::warn!(%import_id,reason=error.1,"Automatic stack assignment held");
                            continue;
                        }
                    };
                    let copy = selection.clone();
                    let available = match blocking(move || prepare::outputs(&catalog, &plan, &copy))
                        .await?
                    {
                        Ok(available) => available,
                        Err(error) => {
                            tracing::warn!(%import_id,reason=error.1,"Automatic stack transfer held");
                            continue;
                        }
                    };
                    available["outputs"].as_array().and_then(|all|all.iter().find(|o|o["wbpp"]==false && o["matches_selection"]==true))
                        .and_then(|o|serde_json::from_value::<Output>(json!({"job_id":o["job_id"],"group_index":o["group_index"],"revision":o["revision"]})).ok())
                } else {
                    None
                };
                if policy["mode"] == "psf_guard" && output.is_none() {
                    continue;
                }
                let request = Request {
                    selection,
                    output,
                    confirm_wbpp_membership: false,
                    automatic: true,
                };
                let value = serde_json::to_value(request).map_err(|_| invalid())?;
                let key = Uuid::new_v5(&id, &serde_json::to_vec(&value).map_err(|_| invalid())?);
                if service
                    .clone()
                    .query(move |s| s.transfer_revision(key))
                    .await?
                    .is_some()
                {
                    continue;
                }
                let now = workflows::now_ms()?;
                service
                    .clone()
                    .run(move |s| s.create_transfer(id, value, now))
                    .await?;
                remaining -= 1;
            }
        }
    }
    Ok(())
}

async fn validate(
    catalog: &Arc<DatabaseContext>,
    import: &psf_guard_director_interop::collaboration::PreparedImport,
    request: &Request,
    package: &Package,
) -> Result<(), Failure> {
    let (catalog, import, request, guard) = (
        catalog.clone(),
        import.clone(),
        request.clone(),
        package.guard.clone(),
    );
    blocking(move|| {
        let (manifest,_,_)=prepare::manifest(&catalog,&import,&request,||false)?;
        if psf_guard_director_interop::collaboration::digest(&serde_json::to_vec(&manifest).map_err(|_|invalid())?)!=guard {
            return Err(Failure(StatusCode::CONFLICT,"Grades, calibration selection or stack provenance changed; prepare and review a new package"));
        }
        Ok(())
    }).await?
}

struct ProgressReader {
    file: tokio::fs::File,
    service: Arc<Service>,
}
impl AsyncRead for ProgressReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.file).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result
            && let Ok(mut progress) = self.service.collaboration.transfers.lock()
        {
            progress.bytes = progress
                .bytes
                .saturating_add((buf.filled().len() - before) as u64);
        }
        result
    }
}

async fn upload(
    state: &Arc<AppState>,
    service: Arc<Service>,
    job: &Job,
    b: &ConnectionBinding,
    package: &Package,
    artifact: &Artifact,
    token: &str,
) -> Result<bool, Failure> {
    let (root, source, expected, shutdown) = (
        package.staging_root.clone(),
        artifact.path.clone(),
        artifact.clone(),
        state.shutdown.clone(),
    );
    // Only one file is staged at a time. The immutable copy prevents a save or
    // external edit from changing bytes after their content name was chosen.
    let staged = blocking(move || stage(&root, &source, &expected, &shutdown)).await??;
    let id = job.id;
    if service
        .clone()
        .query(move |s| s.transfer_revision(id))
        .await?
        != Some(job.revision)
    {
        return Ok(false);
    }
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .read_timeout(Duration::from_secs(60))
        .timeout(Duration::from_secs(3600))
        .build()
        .map_err(|_| invalid())?;
    let mut url = reqwest::Url::parse(&b.base_url).map_err(|_| invalid())?;
    {
        let mut path = url.path_segments_mut().map_err(|_| invalid())?;
        path.pop_if_empty().extend([
            "api",
            "v1",
            "projects",
            &package.project,
            "files",
            &artifact.name,
        ]);
    }
    url.query_pairs_mut()
        .append_pair("kind", &artifact.kind)
        .append_pair("telescope", &b.name.chars().take(60).collect::<String>());
    let file = tokio::fs::File::open(staged.path())
        .await
        .map_err(|_| Failure(StatusCode::CONFLICT, "A prepared file is unavailable"))?;
    *service
        .collaboration
        .transfers
        .lock()
        .map_err(|_| invalid())? = Progress {
        job_id: Some(job.id),
        file: artifact.name.clone(),
        bytes: 0,
    };
    let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(ProgressReader {
        file,
        service: service.clone(),
    }));
    let send = async {
        let mut response = client
            .put(url)
            .bearer_auth(token)
            .header("content-type", "application/octet-stream")
            .header("content-length", artifact.bytes)
            .body(body)
            .send()
            .await
            .map_err(|_| {
                Failure(
                    StatusCode::BAD_GATEWAY,
                    "Upload interrupted; the same content-addressed file will be retried",
                )
            })?;
        if !response.status().is_success() {
            return Err(match response.status() {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Failure(
                    StatusCode::CONFLICT,
                    "Uploads require a valid telescope token associated with a Discord owner",
                ),
                StatusCode::TOO_MANY_REQUESTS => Failure(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Upload rate limited; retry scheduled",
                ),
                StatusCode::INSUFFICIENT_STORAGE => Failure(
                    StatusCode::INSUFFICIENT_STORAGE,
                    "Upload server has insufficient storage; retry scheduled",
                ),
                status if status.is_server_error() => Failure(
                    StatusCode::BAD_GATEWAY,
                    "Upload server unavailable; retry scheduled",
                ),
                _ => Failure(
                    StatusCode::CONFLICT,
                    "Upload refused; check project collection settings and file limits",
                ),
            });
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| {
            Failure(
                StatusCode::BAD_GATEWAY,
                "Upload acknowledgment interrupted; retry retained",
            )
        })? {
            if bytes.len() + chunk.len() > 262_144 {
                return Err(invalid());
            }
            bytes.extend_from_slice(&chunk);
        }
        let ack: Value = serde_json::from_slice(&bytes).map_err(|_| {
            Failure(
                StatusCode::BAD_GATEWAY,
                "Upload acknowledgment unreadable; retry retained",
            )
        })?;
        if ack["file"]["name"] != artifact.name
            || ack["file"]["kind"] != artifact.kind
            || ack["file"]["bytes"].as_u64() != Some(artifact.bytes)
            || ack["file"]["user"].as_str() != package.owner.as_deref()
        {
            return Err(Failure(
                StatusCode::BAD_GATEWAY,
                "Upload acknowledgment does not match the prepared file",
            ));
        }
        Ok(true)
    };
    tokio::pin!(send);
    let mut check = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            result=&mut send=>return result,
            _=state.shutdown.begun()=>return Err(Failure(StatusCode::SERVICE_UNAVAILABLE,"Transfer stopped for application shutdown")),
            _=check.tick()=>{
                let id=job.id;
                if service.clone().query(move|s|s.transfer_revision(id)).await?!=Some(job.revision) {return Ok(false);}
            }
        }
    }
}

fn stage(
    root: &std::path::Path,
    source: &std::path::Path,
    expected: &Artifact,
    shutdown: &crate::server::shutdown::Shutdown,
) -> Result<tempfile::NamedTempFile, Failure> {
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    std::fs::create_dir_all(root).map_err(|_| {
        Failure(
            StatusCode::INSUFFICIENT_STORAGE,
            "Cannot prepare an upload snapshot in the catalog cache",
        )
    })?;
    let mut snapshot = tempfile::Builder::new()
        .prefix("transfer-")
        .suffix(".part")
        .tempfile_in(root)
        .map_err(|_| invalid())?;
    let mut input = std::fs::File::open(source).map_err(|_| {
        Failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "Source file is temporarily unavailable; retry retained",
        )
    })?;
    let mut hash = Sha256::new();
    let mut count = 0u64;
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        if shutdown.is_stopping() {
            return Err(Failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "File snapshot stopped for application shutdown",
            ));
        }
        let n = input.read(&mut buffer).map_err(|_| invalid())?;
        if n == 0 {
            break;
        }
        count = count.saturating_add(n as u64);
        if count > expected.bytes {
            return Err(Failure(
                StatusCode::CONFLICT,
                "Source file grew; prepare and review again",
            ));
        }
        hash.update(&buffer[..n]);
        snapshot.write_all(&buffer[..n]).map_err(|_| {
            Failure(
                StatusCode::INSUFFICIENT_STORAGE,
                "Insufficient local space for an upload snapshot",
            )
        })?;
    }
    let hash = hash
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if count != expected.bytes || hash != expected.hash {
        return Err(Failure(
            StatusCode::CONFLICT,
            "Source bytes changed; prepare and review again",
        ));
    }
    snapshot.flush().map_err(|_| invalid())?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::director::collaboration_auth::tests::{add, fixture, mock};
    use axum::{
        body::{to_bytes, Body},
        http::Request as HttpRequest,
        routing::put,
    };

    async fn upload_fixture(
        reply: Router,
    ) -> (
        tempfile::TempDir,
        Arc<AppState>,
        Arc<Service>,
        Job,
        ConnectionBinding,
        Package,
        Artifact,
        tokio::task::JoinHandle<()>,
    ) {
        let (url, server) = mock(reply).await;
        let (dir, state, app, rig) = fixture().await;
        let connection = add(&app, rig, &url, &[]).await;
        let service = state.director.clone().unwrap();
        let b = binding(service.clone(), connection).await.unwrap();
        let mut job = service
            .clone()
            .run(move |s| s.create_transfer(connection, json!({}), 100))
            .await
            .unwrap();
        job.state = TransferState::Uploading;
        let job = save(service.clone(), job).await.unwrap();
        let path = dir.path().join("raw.fits");
        seiza_fits::write_f32_image(&path, 2, 2, seiza_fits::F32ImageData::Mono(&[1.0; 4]), &[])
            .unwrap();
        let (hash, bytes, modified) = prepare::hash_file(&path).unwrap();
        let artifact = Artifact {
            name: "test.fits".into(),
            kind: "light".into(),
            path,
            hash,
            bytes,
            modified,
        };
        let package = Package {
            staging_root: dir.path().join("staging"),
            owner: Some("owner".into()),
            catalog_id: Uuid::new_v4(),
            rig_id: rig,
            base_url: url,
            agent_id: "test-agent".into(),
            project: "test-project".into(),
            label: "Test".into(),
            catalog: "test".into(),
            manifest: Value::Null,
            guard: String::new(),
            artifacts: vec![artifact.clone()],
            warnings: vec![],
        };
        (dir, state, service, job, b, package, artifact, server)
    }

    #[tokio::test]
    async fn transfer_streams_exact_original_bytes_and_accepts_duplicate_receipt() {
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = count.clone();
        let received = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
        let snapshots = received.clone();
        let remote=Router::new().route("/api/v1/projects/{project}/files/{name}",put(move|request:HttpRequest<Body>|{
            let seen=seen.clone();let snapshots=snapshots.clone();async move {
                assert_eq!(request.headers()["authorization"],"Bearer test-token");
                let length=request.headers()["content-length"].to_str().unwrap().parse::<usize>().unwrap();
                let bytes=to_bytes(request.into_body(),1_048_576).await.unwrap();
                assert_eq!(bytes.len(),length);assert!(bytes.starts_with(b"SIMPLE"));
                snapshots.lock().unwrap().push(bytes.to_vec());
                let already=seen.fetch_add(1,std::sync::atomic::Ordering::SeqCst)>0;
                Json(json!({"file":{"name":"test.fits","kind":"light","bytes":length,"user":"owner"},"already":already}))
            }
        }));
        let (_dir, state, service, job, b, package, artifact, server) =
            upload_fixture(remote).await;
        let before = std::fs::read(&artifact.path).unwrap();
        assert!(upload(
            &state,
            service.clone(),
            &job,
            &b,
            &package,
            &artifact,
            "test-token"
        )
        .await
        .unwrap());
        assert!(
            upload(&state, service, &job, &b, &package, &artifact, "test-token")
                .await
                .unwrap()
        );
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 2);
        assert_eq!(
            *received.lock().unwrap(),
            vec![before.clone(), before.clone()]
        );
        assert_eq!(std::fs::read(&artifact.path).unwrap(), before);
        server.abort();
        server.await.ok();
    }

    #[tokio::test]
    #[ignore = "Requires STARFRONT_REFERENCE and STARFRONT_PYTHON for an isolated upstream upload server"]
    async fn upstream_starfront_upload_receipts_and_duplicate_delivery() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        struct Reference(std::process::Child);
        impl Drop for Reference {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let (dir, state, service, job, mut b, mut package, mut artifact, mock_server) =
            upload_fixture(Router::new()).await;
        mock_server.abort();
        mock_server.await.ok();
        let root = dir.path().join("upstream");
        let mut process = Reference(
            Command::new(std::env::var("STARFRONT_PYTHON").expect("Set Starfront Python path"))
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/tests/support/starfront_upload_server.py"
                ))
                .arg(std::env::var("STARFRONT_REFERENCE").expect("Set Starfront checkout path"))
                .arg(&root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap(),
        );
        let mut line = String::new();
        BufReader::new(process.0.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let remote: Value =
            serde_json::from_str(&line).expect("Starfront upload fixture did not start");
        let token = std::fs::read_to_string(remote["token_file"].as_str().unwrap()).unwrap();
        b.base_url = remote["url"].as_str().unwrap().into();
        b.agent_id = Some(remote["agent"].as_str().unwrap().into());
        let client = Remote::new(&b).unwrap();
        let projects: Value = serde_json::from_slice(
            &client
                .get("files/projects", Some(&token), None)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(projects["signedIn"], true);
        let project = remote["project"].as_str().unwrap();
        let listing = format!("projects/{project}/files");
        let before: Value =
            serde_json::from_slice(&client.get(&listing, Some(&token), None).await.unwrap())
                .unwrap();
        assert_eq!(before["me"], remote["owner"]);
        assert!(before["files"].as_array().unwrap().is_empty());
        package.project = project.into();
        package.owner = before["me"].as_str().map(str::to_owned);
        artifact.name = format!("psfg-light-{}.fits", artifact.hash);
        for _ in 0..2 {
            assert!(upload(
                &state,
                service.clone(),
                &job,
                &b,
                &package,
                &artifact,
                &token
            )
            .await
            .unwrap());
        }
        let after: Value =
            serde_json::from_slice(&client.get(&listing, Some(&token), None).await.unwrap())
                .unwrap();
        let files = after["files"].as_array().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["name"], artifact.name);
        assert_eq!(files[0]["bytes"], artifact.bytes);
        let received = root
            .join("files")
            .join(project)
            .join(remote["owner"].as_str().unwrap())
            .join("lights")
            .join(&artifact.name);
        assert_eq!(
            std::fs::read(received).unwrap(),
            std::fs::read(&artifact.path).unwrap()
        );
    }

    #[tokio::test]
    async fn wrong_acknowledgment_is_not_marked_delivered() {
        let remote = Router::new().route(
            "/api/v1/projects/{project}/files/{name}",
            put(|| async { Json(json!({"file":{"name":"wrong.fits","kind":"light","bytes":1}})) }),
        );
        let (_dir, state, service, job, b, package, artifact, server) =
            upload_fixture(remote).await;
        assert!(upload(
            &state,
            service.clone(),
            &job,
            &b,
            &package,
            &artifact,
            "test-token"
        )
        .await
        .is_err());
        assert!(service
            .query(move |s| s.transfer(job.id))
            .await
            .unwrap()
            .unwrap()
            .completed
            .is_empty());
        server.abort();
        server.await.ok();
    }

    #[tokio::test]
    async fn paused_work_and_changed_sources_never_start_an_upload() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let remote = Router::new().route(
            "/api/v1/projects/{project}/files/{name}",
            put(move || {
                let seen = seen.clone();
                async move {
                    seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Json(json!({}))
                }
            }),
        );
        let (_dir, state, service, job, b, package, artifact, server) =
            upload_fixture(remote).await;
        let id = job.id;
        let revision = job.revision;
        service
            .clone()
            .run(move |s| s.control_transfer(id, revision, "pause"))
            .await
            .unwrap();
        assert!(!upload(
            &state,
            service.clone(),
            &job,
            &b,
            &package,
            &artifact,
            "test-token"
        )
        .await
        .unwrap());
        std::fs::write(&artifact.path, b"Changed after review").unwrap();
        assert!(
            upload(&state, service, &job, &b, &package, &artifact, "test-token")
                .await
                .is_err()
        );
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(std::fs::read_dir(&package.staging_root).unwrap().count(), 0);
        server.abort();
        server.await.ok();
    }

    #[tokio::test]
    async fn shutdown_interrupts_a_stalled_upload_without_waiting_for_its_timeout() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let ready = entered.clone();
        let remote = Router::new().route(
            "/api/v1/projects/{project}/files/{name}",
            put(move || {
                let ready = ready.clone();
                async move {
                    ready.notify_one();
                    tokio::time::sleep(Duration::from_secs(120)).await;
                    Json(json!({}))
                }
            }),
        );
        let (_dir, state, service, job, b, package, artifact, server) =
            upload_fixture(remote).await;
        let stop = state.shutdown.clone();
        let task = tokio::spawn(async move {
            entered.notified().await;
            stop.begin();
        });
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            upload(
                &state,
                service.clone(),
                &job,
                &b,
                &package,
                &artifact,
                "test-token",
            ),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        assert!(service
            .query(move |s| s.transfer(job.id))
            .await
            .unwrap()
            .unwrap()
            .completed
            .is_empty());
        task.await.unwrap();
        server.abort();
        server.await.ok();
    }
}
