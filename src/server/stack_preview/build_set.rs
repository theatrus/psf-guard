//! The build set: what Build stacks and Build channel ask for, prepared
//! into stack builds. Preparing reads the project's frames, scores them and
//! matches each light to the calibration library, then queues each build,
//! or hands back the same build already queued, running or finished.

use std::collections::VecDeque;

use super::*;

/// `POST /api/db/{db}/projects/{project}/stack-previews` — one build of
/// the request's images, or of the one `channel` it names. It joins the
/// line at once, as a build being prepared: `202` and that build, which a
/// client follows through `replaced_by` once it is prepared.
pub async fn start_stack_previews(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(request): Json<StackPreviewRequest>,
) -> Result<(StatusCode, Json<ApiResponse<StackPreviewJob>>), AppError> {
    validate_request(&request)?;
    let job = queue_preparation(&state, &ctx.0, project_id, request, false)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(job.for_client())),
    ))
}

/// `POST /api/db/{db}/projects/{project}/stack-previews/channels` — the
/// request's images split into one build per channel, queued in channel
/// order so the header's queue lists each channel on its own. Each carries
/// its siblings' reference frames, so an unsolved channel still faces the
/// way the solved ones do; a request naming one `channel` on the single
/// route makes the same build, so the two share a cache. Channels with too
/// few frames come back together as one build that settles at once, and a
/// channel already built for this request comes back as it is.
///
/// The split happens as the request is prepared, off the request: `202` and
/// one build being prepared, whose `replaced_by` names the channel builds.
pub async fn start_stack_preview_channels(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(request): Json<StackPreviewRequest>,
) -> Result<(StatusCode, Json<ApiResponse<Vec<StackPreviewJob>>>), AppError> {
    validate_request(&request)?;
    let job = queue_preparation(&state, &ctx.0, project_id, request, true)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(ApiResponse::success(vec![job.for_client()])),
    ))
}

/// Builds a person asked for that wait to be prepared: one line and one
/// runner per catalog, so a long preparation in one catalog never holds up
/// another's.
#[derive(Default)]
pub(super) struct PreparationLine {
    catalogs: HashMap<String, CatalogLine>,
    /// The build each request waiting or being prepared became, by
    /// request, so the same click twice joins one build.
    by_request: HashMap<String, String>,
}

/// One catalog's requests, in click order, and whether its runner is
/// preparing them.
#[derive(Default)]
struct CatalogLine {
    pending: VecDeque<Preparation>,
    running: bool,
}

struct Preparation {
    job_id: String,
    request_key: String,
    ctx: Arc<DatabaseContext>,
    project_id: i32,
    request: StackPreviewRequest,
    /// One build per channel (Build stacks), or one build.
    split: bool,
    cancel: Arc<AtomicBool>,
}

/// Put a person's build in the line as being prepared, and see that a
/// runner prepares it. Nothing is read from the catalog or storage here.
pub(super) fn queue_preparation(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    project_id: i32,
    request: StackPreviewRequest,
    split: bool,
) -> Result<StackPreviewJob, AppError> {
    let manager = &state.stack_previews;
    let request_key = preparation_key(&ctx.id, project_id, &request, split);
    let mut line = manager.preparing.lock().unwrap();
    if let Some(job) = line
        .by_request
        .get(&request_key)
        .and_then(|job_id| manager.get(job_id))
        .filter(|job| job.state == StackJobState::Preparing)
    {
        return Ok(job);
    }
    let job = preparing_job(&ctx.id, project_id, &request, &request_key);
    if !manager.insert(job.clone()) {
        return Err(AppError::BadRequest(format!(
            "At most {MAX_QUEUED_JOBS} stack builds may wait at once, and \
             {MAX_REMEMBERED_JOBS} may be running or kept"
        )));
    }
    let cancel = manager.track_cancel(&job.job_id);
    line.by_request
        .insert(request_key.clone(), job.job_id.clone());
    let catalog = line.catalogs.entry(ctx.id.clone()).or_default();
    catalog.pending.push_back(Preparation {
        job_id: job.job_id.clone(),
        request_key,
        ctx: Arc::clone(ctx),
        project_id,
        request,
        split,
        cancel,
    });
    if !catalog.running {
        catalog.running = true;
        drop(line);
        start_preparing(state, &ctx.id);
    }
    Ok(job)
}

/// What identifies a request: the same click twice is one build.
fn preparation_key(
    database_id: &str,
    project_id: i32,
    request: &StackPreviewRequest,
    split: bool,
) -> String {
    let mut request = request.clone();
    request.image_ids.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update(database_id.as_bytes());
    hasher.update(project_id.to_le_bytes());
    hasher.update([u8::from(split)]);
    hasher.update(serde_json::to_vec(&request).unwrap_or_default());
    hex_digest(hasher)
}

/// The build a request is while it is prepared. Its id is its own, not the
/// id the prepared build gets: that one depends on what preparing reads.
fn preparing_job(
    database_id: &str,
    project_id: i32,
    request: &StackPreviewRequest,
    request_key: &str,
) -> StackPreviewJob {
    let mut hasher = Sha256::new();
    hasher.update(request_key.as_bytes());
    hasher.update(new_artifact_revision().as_bytes());
    let frames = format!("{} frames", request.image_ids.len());
    let label = match &request.channel {
        Some(channel) if !channel.filter_name.is_empty() => {
            format!("{} · {frames}", channel.filter_name)
        }
        _ => frames,
    };
    StackPreviewJob {
        schema_version: 2,
        job_id: hex_digest(hasher),
        database_id: database_id.to_string(),
        project_id,
        state: StackJobState::Preparing,
        accepted_only: request.accepted_only,
        created_unix_seconds: chrono::Utc::now().timestamp(),
        artifact_revision: String::new(),
        cache_version: STACK_PREVIEW_CACHE_VERSION,
        stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
        order: request.order,
        scoring: StackScoringSettings::from_overrides(&request.scoring),
        method: None,
        groups: Vec::new(),
        error: None,
        color_defaults: request.color_defaults.clone(),
        automatic: false,
        preparing: Some(label),
        replaced_by: Vec::new(),
    }
}

/// A catalog's runner thread prepares its line, in click order, until it
/// is empty. It runs on the async runtime's handle: queueing a prepared
/// build spawns its task there.
fn start_preparing(state: &Arc<AppState>, database_id: &str) {
    let runner_state = Arc::clone(state);
    let runner_database = database_id.to_string();
    let handle = tokio::runtime::Handle::current();
    let spawned = std::thread::Builder::new()
        .name("stack-preparation".into())
        .spawn(move || {
            let _runtime = handle.enter();
            serve_preparations(&runner_state, &runner_database);
        });
    if let Err(error) = spawned {
        tracing::warn!("could not start preparing stack builds: {error}");
        let pending = {
            let mut line = state.stack_previews.preparing.lock().unwrap();
            line.catalogs
                .remove(database_id)
                .map(|catalog| catalog.pending)
                .unwrap_or_default()
        };
        for preparation in pending {
            end_preparation(
                state,
                &preparation,
                Err(format!("Could not start preparing the build: {error}")),
            );
        }
    }
}

/// Frees a catalog's line if its runner ends any way but an empty line, so
/// the next click starts another rather than waiting on a runner that is
/// gone. What waits stays in the line for it.
struct RunnerClaim<'a> {
    state: &'a AppState,
    database_id: &'a str,
    released: bool,
}

impl Drop for RunnerClaim<'_> {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let mut line = self
            .state
            .stack_previews
            .preparing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(catalog) = line.catalogs.get_mut(self.database_id) {
            catalog.running = false;
        }
    }
}

fn serve_preparations(state: &Arc<AppState>, database_id: &str) {
    let mut claim = RunnerClaim {
        state,
        database_id,
        released: false,
    };
    loop {
        let Some(preparation) = ({
            let mut line = state.stack_previews.preparing.lock().unwrap();
            let catalog = line.catalogs.entry(database_id.to_string()).or_default();
            let next = catalog.pending.pop_front();
            if next.is_none() {
                catalog.running = false;
                claim.released = true;
            }
            next
        }) else {
            return;
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prepare_and_queue(state, &preparation)
        }))
        .unwrap_or_else(|_| Err("Preparing the build failed unexpectedly".into()));
        end_preparation(state, &preparation, outcome);
    }
}

/// Whether a preparation was stopped, or the server is stopping.
fn preparation_stopped(state: &AppState, preparation: &Preparation) -> bool {
    preparation.cancel.load(Ordering::Relaxed) || state.shutdown.is_stopping()
}

/// Prepare one request and queue what it became, or hand back the same
/// builds already queued, running or finished. `None` when it was stopped.
fn prepare_and_queue(
    state: &Arc<AppState>,
    preparation: &Preparation,
) -> Result<Option<Vec<String>>, String> {
    if preparation_stopped(state, preparation) {
        return Ok(None);
    }
    // A person is waiting on it, and the server's stop waits for it.
    let _person = state.begin_named_interactive_job("preparing a stack build");
    let _hold = state.shutdown.hold("preparing a stack build");
    let Preparation {
        ctx,
        project_id,
        request,
        ..
    } = preparation;
    let parts = if preparation.split {
        prepare_channels(ctx, *project_id, request, None)
    } else {
        prepare_job(ctx, *project_id, request).map(|prepared| vec![(request.clone(), prepared)])
    }
    .map_err(|error| error.message())?;
    if preparation_stopped(state, preparation) {
        return Ok(None);
    }
    let mut job_ids = Vec::with_capacity(parts.len());
    // The builds this request put in the line itself, not ones already there.
    let mut queued = Vec::new();
    let mut cached = Vec::new();
    for (part_request, prepared) in parts {
        if preparation_stopped(state, preparation) {
            break;
        }
        let new = state.stack_previews.get(&prepared.public.job_id).is_none();
        let started = start_prepared(state, ctx, *project_id, &part_request, prepared)
            .map_err(|error| error.message())?;
        if new {
            queued.push(started.job.job_id.clone());
        }
        job_ids.push(started.job.job_id);
        cached.extend(started.cached);
    }
    // A stop that came while the builds were queued stops the ones this
    // request queued; builds that were already in the line keep going.
    if preparation_stopped(state, preparation) {
        for job_id in &queued {
            state.stack_previews.request_cancel(job_id);
        }
        return Ok(None);
    }
    // Once every channel is in line, so color waits for the ones still to
    // build rather than composing from a half-queued set.
    for job in cached {
        automatic::compose_colors_after(state, ctx, &job);
    }
    Ok(Some(job_ids))
}

/// Settle the build a request was while it was prepared: replaced by what it
/// became, failed with why, or stopped.
fn end_preparation(
    state: &Arc<AppState>,
    preparation: &Preparation,
    outcome: Result<Option<Vec<String>>, String>,
) {
    let manager = &state.stack_previews;
    manager.update(&preparation.job_id, |job| {
        job.preparing = None;
        match outcome {
            // Builds already queued are the person's to follow or stop, even
            // when the stop came as they were queued.
            Ok(Some(job_ids)) => {
                job.state = StackJobState::Replaced;
                job.replaced_by = job_ids;
            }
            _ if job.state == StackJobState::Cancelled => {}
            Ok(None) => job.state = StackJobState::Cancelled,
            Err(error) => {
                job.state = StackJobState::Failed;
                job.error = Some(error);
            }
        }
    });
    manager.forget_cancel_of(&preparation.job_id, &preparation.cancel);
    let mut line = manager.preparing.lock().unwrap();
    if line.by_request.get(&preparation.request_key) == Some(&preparation.job_id) {
        line.by_request.remove(&preparation.request_key);
    }
}

pub(super) struct Started {
    pub(super) job: StackPreviewJob,
    /// A completed build the request matched, whose color may need to
    /// follow it: its card can point back at an older build.
    pub(super) cached: Option<StackPreviewJob>,
}

/// Queue a prepared build, or hand back the same build already queued,
/// running, or finished.
pub(super) fn start_prepared(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    project_id: i32,
    request: &StackPreviewRequest,
    prepared: PreparedJob,
) -> Result<Started, AppError> {
    let manifest_path = manifest_path(&prepared.stack_root, &prepared.public.job_id);
    if let Some(existing) = state.stack_previews.get(&prepared.public.job_id)
        && (matches!(
            existing.state,
            StackJobState::Queued | StackJobState::Running
        ) || (!request.force && existing.state == StackJobState::Completed))
    {
        if existing.state == StackJobState::Completed
            && let Err(error) = persist_latest_and_reapply(state, ctx, &existing)
        {
            tracing::warn!("Failed to refresh latest stack preview index: {error}");
        }
        // The same build is already under way for the automatic refresh:
        // it is now the user's, and every other automatic build yields. It
        // takes the person's color defaults with it.
        let existing = if existing.automatic {
            state.stack_previews.adopt(&existing.job_id);
            if request.color_defaults.is_some() {
                let defaults = request.color_defaults.clone();
                state.stack_previews.update(&existing.job_id, |job| {
                    job.color_defaults = defaults;
                });
            }
            interrupt_automatic(state);
            state
                .stack_previews
                .get(&existing.job_id)
                .unwrap_or(existing)
        } else {
            existing
        };
        // A finished build whose previews are out of date renders them
        // again from its FITS rather than stacking again.
        if existing.state == StackJobState::Completed
            && let Some(rendering) =
                queue_preview_render(state, ctx, &existing, false, request.color_defaults.clone())
        {
            return Ok(Started {
                job: rendering,
                cached: None,
            });
        }
        let cached = (existing.state == StackJobState::Completed).then(|| {
            let mut cached = existing.clone();
            cached.color_defaults = request.color_defaults.clone();
            cached
        });
        return Ok(Started {
            job: existing,
            cached,
        });
    }
    if !request.force
        && let Ok(bytes) = std::fs::read(&manifest_path)
        && let Ok(existing) = serde_json::from_slice::<StackPreviewJob>(&bytes)
        && existing.state == StackJobState::Completed
    {
        if let Err(error) = persist_latest_and_reapply(state, ctx, &existing) {
            tracing::warn!("Failed to refresh latest stack preview index: {error}");
        }
        let _ = state.stack_previews.insert(existing.clone());
        if let Some(rendering) =
            queue_preview_render(state, ctx, &existing, false, request.color_defaults.clone())
        {
            return Ok(Started {
                job: rendering,
                cached: None,
            });
        }
        let mut cached = existing.clone();
        cached.color_defaults = request.color_defaults.clone();
        return Ok(Started {
            job: existing,
            cached: Some(cached),
        });
    }

    // Real work is about to queue: the user's build takes the worker.
    interrupt_automatic(state);
    let response = prepared.public.clone();
    if !state.stack_previews.insert(response.clone()) {
        return Err(AppError::BadRequest(format!(
            "At most {MAX_QUEUED_JOBS} stack builds may wait at once, and \
             {MAX_REMEMBERED_JOBS} may be running or kept"
        )));
    }
    let origin = journal::JournaledStackJob::mono(&ctx.id, project_id, request, &response, false);
    enqueue_job(Arc::clone(state), prepared, origin);
    Ok(Started {
        job: response,
        cached: None,
    })
}

pub(super) fn validate_request(request: &StackPreviewRequest) -> Result<(), AppError> {
    if request.image_ids.len() < 2 {
        return Err(AppError::BadRequest(
            "Stack previews require at least two image IDs".into(),
        ));
    }
    if request.image_ids.len() > MAX_REQUEST_IMAGES {
        return Err(AppError::BadRequest(format!(
            "Stack preview requests are limited to {MAX_REQUEST_IMAGES} images"
        )));
    }
    let unique = request.image_ids.iter().copied().collect::<HashSet<_>>();
    if unique.len() != request.image_ids.len() {
        return Err(AppError::BadRequest(
            "Stack preview image IDs must be unique".into(),
        ));
    }
    Ok(())
}

pub(super) fn prepare_job(
    ctx: &Arc<DatabaseContext>,
    project_id: i32,
    request: &StackPreviewRequest,
) -> Result<PreparedJob, AppError> {
    let Some(channel) = &request.channel else {
        return prepare_whole(ctx, project_id, request, None).map(|(job, _)| job);
    };
    prepare_channels(
        ctx,
        project_id,
        request,
        Some(std::slice::from_ref(channel)),
    )?
    .pop()
    .map(|(_, job)| job)
    .ok_or_else(|| AppError::BadRequest("No image in this request is in that channel".into()))
}

/// What one group hashes to, so its build can stand alone: the shared
/// settings and that group's own frames.
pub(super) struct GroupIdentity {
    key: StackChannelKey,
    hash: String,
}

/// The request split into one build per channel, each with the reference
/// frames of the same target's other channels for the pier-side vote.
/// Their image ids and source fingerprints are part of each build's
/// identity, since they can change which way an unsolved channel faces;
/// only the same target's channels count, so a build is the same whether
/// the request covered the project or that target. With `only`, just those
/// channels, even ones with too few frames to stack; without it, channels
/// with too few frames come back together in one build that settles at once.
pub(super) fn prepare_channels(
    ctx: &Arc<DatabaseContext>,
    project_id: i32,
    request: &StackPreviewRequest,
    only: Option<&[StackChannelKey]>,
) -> Result<Vec<(StackPreviewRequest, PreparedJob)>, AppError> {
    let mut whole_request = request.clone();
    whole_request.channel = None;
    let (whole, identities) = prepare_whole(ctx, project_id, &whole_request, only)?;
    let buildable = whole
        .groups
        .iter()
        .map(|group| group.frames.len() >= 2)
        .collect::<Vec<_>>();
    let peer_frames = whole
        .groups
        .iter()
        .map(|group| group.frames.first().cloned())
        .collect::<Vec<_>>();
    let targets = identities
        .iter()
        .map(|identity| identity.key.target_id)
        .collect::<Vec<_>>();
    let PreparedJob {
        public,
        groups,
        orientation_peers: _,
        stack_root,
        calibration_root,
        north_up,
        order,
    } = whole;
    let mut channels = Vec::new();
    let mut skipped_images = Vec::new();
    for (((group, mut status), identity), position) in groups
        .into_iter()
        .zip(public.groups.iter().cloned())
        .zip(identities)
        .zip(0..)
    {
        if let Some(only) = only
            && !only.contains(&identity.key)
        {
            continue;
        }
        if only.is_none() && !buildable[position] {
            skipped_images.extend(status.input_images.iter().map(|image| image.image_id));
            continue;
        }
        let peers = peer_frames
            .iter()
            .enumerate()
            .filter(|(other, _)| {
                *other != position && buildable[*other] && targets[*other] == identity.key.target_id
            })
            .filter_map(|(_, frame)| frame.clone())
            .collect::<Vec<_>>();
        let mut hasher = Sha256::new();
        hasher.update(identity.hash.as_bytes());
        hasher.update(b"\0orientation-peers\0");
        for peer in &peers {
            hasher.update(peer.image_id.to_le_bytes());
            hasher.update(peer.source_fingerprint.as_bytes());
        }
        let job_id = hex_digest(hasher);
        status.index = 0;
        if status.state == StackGroupState::Queued {
            status.preview_url = Some(format!(
                "/api/db/{}/stack-previews/{job_id}/0/preview?v={}",
                ctx.id, public.artifact_revision
            ));
            status.fits_url = Some(format!(
                "/api/db/{}/stack-previews/{job_id}/0/fits?v={}",
                ctx.id, public.artifact_revision
            ));
        }
        let mut channel_public = public.clone();
        channel_public.job_id = job_id;
        channel_public.groups = vec![status];
        let mut channel_request = whole_request.clone();
        channel_request.channel = Some(identity.key);
        channels.push((
            channel_request,
            PreparedJob {
                public: channel_public,
                groups: vec![PreparedGroup { index: 0, ..group }],
                orientation_peers: peers,
                stack_root: stack_root.clone(),
                calibration_root: calibration_root.clone(),
                north_up,
                order,
            },
        ));
    }
    if !skipped_images.is_empty() {
        let mut skipped_request = whole_request.clone();
        skipped_request.image_ids = skipped_images;
        let skipped = prepare_whole(ctx, project_id, &skipped_request, None)?.0;
        channels.push((skipped_request, skipped));
    }
    Ok(channels)
}

/// Prepare every channel of the request as one build, and say what each
/// group hashes to on its own. With `fingerprint_only`, the costly
/// calibration fingerprint is taken for that channel alone: the others only
/// lend their reference frames.
pub(super) fn prepare_whole(
    ctx: &Arc<DatabaseContext>,
    project_id: i32,
    request: &StackPreviewRequest,
    fingerprint_only: Option<&[StackChannelKey]>,
) -> Result<(PreparedJob, Vec<GroupIdentity>), AppError> {
    let flat_star_masking = crate::calibration::flat_star_masking_enabled();
    let scoring = StackScoringSettings::from_overrides(&request.scoring);
    let requested = request.image_ids.iter().copied().collect::<HashSet<_>>();
    let (project_images, expected_by_image, mapped_sources, exposure_groups) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);
        let exposure_groups =
            crate::server::exposure_groups::cached_project_groups(ctx, &conn, project_id)?;
        let images = db
            .get_images_by_project_id(project_id)
            .map_err(AppError::db)?;
        if images.is_empty() {
            return Err(AppError::BadRequest(format!(
                "Project {project_id} has no images"
            )));
        }
        let found = images
            .iter()
            .filter(|(image, _, _)| requested.contains(&image.id))
            .count();
        if found != requested.len() {
            return Err(AppError::BadRequest(
                "Every requested image must belong to the selected project".into(),
            ));
        }
        let selected_groups = images
            .iter()
            .filter(|(image, _, _)| requested.contains(&image.id))
            .map(|(image, _, _)| (image.target_id, image.filter_name.clone()))
            .collect::<HashSet<_>>();
        let relevant = images
            .into_iter()
            .filter(|(image, _, _)| {
                selected_groups.contains(&(image.target_id, image.filter_name.clone()))
            })
            .collect::<Vec<_>>();
        let mut resolver = FramingResolver::new(&conn).map_err(AppError::db)?;
        let expected = relevant
            .iter()
            .map(|(image, _, _)| {
                resolver
                    .expected_for_grading(&conn, image)
                    .map(|value| (image.id, value))
            })
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(AppError::db)?;
        let mapped_sources = crate::server::remote_upload::mapped_light_sources(
            &conn,
            &ctx.image_dir_paths,
            relevant.iter().map(|(image, _, _)| image),
        )?;
        (relevant, expected, mapped_sources, exposure_groups)
    };

    let quality = quality_results(
        ctx,
        &project_images,
        &expected_by_image,
        &mapped_sources,
        &scoring,
    );
    let quality_by_id = quality
        .into_iter()
        .map(|result| (result.image_id, result))
        .collect::<HashMap<_, _>>();

    type StackGroupKey = (i32, String, String, Option<String>);
    type ScoredImages = Vec<(AcquiredImage, ImageQualityResult)>;
    let mut grouped: BTreeMap<StackGroupKey, ScoredImages> = BTreeMap::new();
    for (image, _project_name, target_name) in project_images {
        if !requested.contains(&image.id) {
            continue;
        }
        let scored = quality_by_id
            .get(&image.id)
            .cloned()
            .unwrap_or_else(|| fallback_quality(image.id));
        grouped
            .entry((
                image.target_id,
                target_name,
                image.filter_name.clone(),
                exposure_groups
                    .by_image
                    .get(&image.id)
                    .map(|group| group.key.clone()),
            ))
            .or_default()
            .push((image, scored));
    }

    let mut public_groups = Vec::new();
    let mut prepared_groups = Vec::new();
    let artifact_revision = new_artifact_revision();
    let method = request.method.unwrap_or_else(method::current);
    let mut hasher = Sha256::new();
    if exposure_groups.settings.split_exposure_groups {
        hasher.update(b"exposure-groups-v1\0");
    }
    hasher.update(ctx.id.as_bytes());
    hasher.update(project_id.to_le_bytes());
    hasher.update([request.accepted_only as u8]);
    hasher.update([request.north_up as u8]);
    hasher.update(STACK_PREVIEW_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STACKING_OUTPUT_VERSION.as_bytes());
    hasher.update(seiza_stacking::SKY_ORIENTATION_VERSION.to_le_bytes());
    hasher.update(seiza_stacking::SKY_ORIENTATION_NAME.as_bytes());
    hasher.update(PREVIEW_MAX_DIMENSION.to_le_bytes());
    hasher.update(JOB_ID_STRETCH_VERSION.as_bytes());
    hasher.update(request.order.as_str().as_bytes());
    // Only a non-default scoring policy enters the id. Artifacts built before
    // the policy was recorded were built at the calibrated defaults, and this
    // keeps their ids — and their caches — valid rather than forcing every
    // user through one full re-integration on upgrade.
    if scoring != StackScoringSettings::default() {
        hasher.update(scoring.penalty_satellite.to_le_bytes());
        hasher.update(scoring.penalty_pointing.to_le_bytes());
        hasher.update(scoring.penalty_temporal.to_le_bytes());
        for limit in [scoring.hfr_reject_above, scoring.star_count_reject_below] {
            hasher.update([limit.is_some() as u8]);
            if let Some(limit) = limit {
                hasher.update(limit.to_le_bytes());
            }
        }
        // The HFR-rise and star-loss scales came later. They enter only
        // when one is off its default, so an artifact built under another
        // non-default policy before they existed keeps its id too.
        if scoring.penalty_hfr_rise != 1.0 || scoring.penalty_star_loss != 1.0 {
            hasher.update(b"relative-metric-scales");
            hasher.update(scoring.penalty_hfr_rise.to_le_bytes());
            hasher.update(scoring.penalty_star_loss.to_le_bytes());
        }
    }
    // The Seiza version above already changed every id when the method
    // became a choice, so the whole method enters every id.
    hasher.update(b"\0method\0");
    hasher.update(method.fingerprint().as_bytes());

    // Each group's own identity starts from the same settings.
    let prefix = hasher.clone();
    let mut identities = Vec::new();
    // Read from the library once, for every group, when the first needs it.
    let mut matcher: Option<crate::calibration::LightMatcher> = None;
    let exclude_uncalibrated = crate::calibration::exclude_uncalibrated_enabled();
    for (index, ((target_id, target_name, filter_name, exposure_group_key), mut entries)) in
        grouped.into_iter().enumerate()
    {
        let mut group_hasher = prefix.clone();
        // Hash the mode each channel actually stacks under, so the same
        // effective configuration lands on the same job whether it came from
        // the request-wide mode or an override.
        let group_calibration =
            request.calibration_for(target_id, &filter_name, exposure_group_key.as_deref());
        let exposure_group = entries
            .first()
            .and_then(|(image, _)| exposure_groups.by_image.get(&image.id))
            .cloned();
        hasher.update(target_id.to_le_bytes());
        group_hasher.update(target_id.to_le_bytes());
        hasher.update(target_name.as_bytes());
        group_hasher.update(target_name.as_bytes());
        hasher.update(filter_name.as_bytes());
        group_hasher.update(filter_name.as_bytes());
        hasher.update(group_calibration.as_str().as_bytes());
        group_hasher.update(group_calibration.as_str().as_bytes());
        if let Some(key) = &exposure_group_key {
            hasher.update(b"\0exposure-group\0");
            group_hasher.update(b"\0exposure-group\0");
            hasher.update(key.as_bytes());
            group_hasher.update(key.as_bytes());
        }
        entries.sort_by_key(|(image, _)| (image.acquired_date.unwrap_or(0), image.id));
        let total_candidates = entries.len();
        let input_images = entries
            .iter()
            .map(|(image, _)| StackInputImage {
                image_id: image.id,
                grading_status: image.grading_status,
            })
            .collect();
        let mut quality_excluded = 0usize;
        let mut missing_files = 0usize;
        let mut decisions = Vec::new();
        let mut frames = Vec::new();

        for (image, scored) in entries {
            hasher.update(image.id.to_le_bytes());
            group_hasher.update(image.id.to_le_bytes());
            hasher.update(image.grading_status.to_le_bytes());
            group_hasher.update(image.grading_status.to_le_bytes());
            hasher.update(image.acquired_date.unwrap_or(0).to_le_bytes());
            group_hasher.update(image.acquired_date.unwrap_or(0).to_le_bytes());
            hasher.update(scored.quality_score.to_le_bytes());
            group_hasher.update(scored.quality_score.to_le_bytes());
            if let Some(reason) = scored.regrade_reason.as_deref() {
                hasher.update(reason.as_bytes());
                group_hasher.update(reason.as_bytes());
            }

            let exclusion = exclusion_reason(&image, &scored, request.accepted_only);
            if let Some(reason) = exclusion {
                quality_excluded += 1;
                decisions.push(excluded_decision(&image, &scored, reason));
                continue;
            }

            let Some(filename) = crate::server::handlers::filename_from_metadata(&image.metadata)
            else {
                missing_files += 1;
                decisions.push(excluded_decision(
                    &image,
                    &scored,
                    "Metadata has no FITS filename".into(),
                ));
                continue;
            };
            let path =
                match crate::server::handlers::find_fits_file(ctx, &image, &target_name, &filename)
                {
                    Ok(path) => path,
                    Err(AppError::Conflict(message)) => return Err(AppError::Conflict(message)),
                    Err(AppError::NotFound) => {
                        missing_files += 1;
                        decisions.push(excluded_decision(
                            &image,
                            &scored,
                            "FITS file was not found".into(),
                        ));
                        continue;
                    }
                    Err(error) => return Err(error),
                };
            let source_fingerprint = source_fingerprint(&path);
            hasher.update(source_fingerprint.as_bytes());
            group_hasher.update(source_fingerprint.as_bytes());
            let exposure_seconds = exposure_seconds_from_metadata(&image.metadata);
            hasher.update(exposure_seconds.to_le_bytes());
            group_hasher.update(exposure_seconds.to_le_bytes());
            frames.push(PreparedFrame {
                image_id: image.id,
                acquired_date: image.acquired_date,
                quality_score: Some(scored.quality_score),
                path,
                source_fingerprint,
                expected_target: expected_by_image.get(&image.id).copied().flatten(),
                exposure_seconds,
            });
        }

        let fingerprinted = fingerprint_only.is_none_or(|keys| {
            keys.iter()
                .any(|key| key.matches(target_id, &filter_name, exposure_group_key.as_deref()))
        });
        // Match each light against the library once: the fingerprint names
        // the masters for the job's identity, and, when the setting asks, a
        // light the library cannot calibrate properly is left out before the
        // reference is chosen, so it never mixes with frames that were. A channel only lending its
        // reference to a sibling's build skips both.
        let mut fingerprints: HashMap<i32, String> = HashMap::new();
        let mut uncalibrated: Vec<(i32, String)> = Vec::new();
        if !frames.is_empty() && fingerprinted {
            let directory_tree = ctx.get_directory_tree().map_err(AppError::db)?;
            // The shared connection is held only to read the library: the
            // header reads below go to storage, which a busy server makes
            // slow, and every other request waits on that connection.
            if matcher.is_none() {
                let conn = ctx.db();
                let conn = conn.lock().map_err(AppError::db)?;
                matcher = Some(
                    crate::calibration::LightMatcher::load(&conn, exclude_uncalibrated)
                        .map_err(AppError::db)?,
                );
            }
            let matcher = matcher.as_mut().expect("the library was read above");
            let gaps = group_calibration != crate::calibration::CalibrationMode::Off
                && exclude_uncalibrated;
            for frame in &frames {
                let (fingerprint, gap) = matcher
                    .light_calibration(&frame.path, Some(&directory_tree), flat_star_masking, gaps)
                    .map_err(AppError::db)?;
                fingerprints.insert(frame.image_id, fingerprint);
                if let Some(reason) = gap {
                    uncalibrated.push((frame.image_id, reason));
                }
            }
        }
        let calibration_excluded = uncalibrated.len();
        if calibration_excluded > 0 {
            let left_out: HashMap<i32, &str> = uncalibrated
                .iter()
                .map(|(image_id, reason)| (*image_id, reason.as_str()))
                .collect();
            frames.retain(|frame| {
                let Some(reason) = left_out.get(&frame.image_id) else {
                    return true;
                };
                decisions.push(StackFrameDecision {
                    quality_score: frame.quality_score,
                    ..excluded_decision_for(frame.image_id, (*reason).to_string())
                });
                false
            });
        }

        frames.sort_by(|left, right| {
            right
                .quality_score
                .unwrap_or(0.0)
                .total_cmp(&left.quality_score.unwrap_or(0.0))
                .then_with(|| left.acquired_date.cmp(&right.acquired_date))
                .then_with(|| left.image_id.cmp(&right.image_id))
        });
        let reference_image_id = frames.first().map(|frame| frame.image_id);
        // The best-graded frame is the registration reference either way. What
        // the order decides is the rest: chronological for a broad capture
        // trend after that reference, or best-first so the curve reads as the
        // frames you would keep before the ones you would throw away.
        if frames.len() > 1 && request.order == snr::StackFrameOrder::Capture {
            frames[1..].sort_by_key(|frame| (frame.acquired_date.unwrap_or(0), frame.image_id));
        }
        // Hashed in the order the frames stack, as before the calibration
        // check existed, so a channel with nothing left out keeps its
        // identity and its cached stack.
        for frame in &frames {
            if let Some(fingerprint) = fingerprints.get(&frame.image_id) {
                hasher.update(fingerprint.as_bytes());
                group_hasher.update(fingerprint.as_bytes());
            }
        }
        for (image_id, reason) in &uncalibrated {
            for digest in [&mut hasher, &mut group_hasher] {
                digest.update(b"\0uncalibrated\0");
                digest.update(image_id.to_le_bytes());
                digest.update(reason.as_bytes());
            }
        }
        identities.push(GroupIdentity {
            key: StackChannelKey {
                target_id,
                filter_name: filter_name.clone(),
                exposure_group_key: exposure_group_key.clone(),
            },
            hash: hex_digest(group_hasher),
        });
        let eligible_frames = frames.len();
        public_groups.push(StackGroupStatus {
            index,
            target_id,
            target_name,
            filter_name,
            exposure_group,
            state: if eligible_frames >= 2 {
                StackGroupState::Queued
            } else {
                StackGroupState::Skipped
            },
            phase: if eligible_frames >= 2 {
                "queued".into()
            } else {
                "skipped".into()
            },
            total_candidates,
            eligible_frames,
            quality_excluded,
            missing_files,
            calibration_excluded,
            processed_frames: 0,
            accepted_frames: 0,
            rejected_frames: 0,
            reused_frames: 0,
            resume_note: None,
            output_channels: 0,
            sky_orientation: None,
            reference_image_id,
            color_calibration: None,
            color_calibration_basis: None,
            color_calibration_outdated: None,
            preview_stretch_version: None,
            preview_revision: None,
            preview_render_failed: None,
            total_exposure_seconds: 0.0,
            preview_url: None,
            fits_url: None,
            snr: None,
            snr_url: None,
            final_pass: None,
            calibration_progress: None,
            error: (eligible_frames < 2).then(|| match uncalibrated.first() {
                Some((_, reason)) => format!(
                    "Fewer than two frames can be calibrated; {calibration_excluded} left out \
                     ({reason})"
                ),
                None => "Fewer than two eligible FITS frames".to_string(),
            }),
            calibration: crate::calibration::AppliedCalibration::default(),
            input_images,
            frames: decisions,
        });
        prepared_groups.push(PreparedGroup {
            flat_star_masking,
            index,
            calibration: group_calibration,
            frames,
        });
    }

    let digest = hasher.finalize();
    let mut job_id = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut job_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    for group in &mut public_groups {
        if group.state == StackGroupState::Queued {
            group.preview_url = Some(format!(
                "/api/db/{}/stack-previews/{}/{}/preview?v={}",
                ctx.id, job_id, group.index, artifact_revision
            ));
            group.fits_url = Some(format!(
                "/api/db/{}/stack-previews/{}/{}/fits?v={}",
                ctx.id, job_id, group.index, artifact_revision
            ));
        }
    }
    let now = chrono::Utc::now().timestamp();
    Ok((
        PreparedJob {
            public: StackPreviewJob {
                automatic: false,
                schema_version: 2,
                job_id,
                database_id: ctx.id.clone(),
                project_id,
                state: StackJobState::Queued,
                accepted_only: request.accepted_only,
                created_unix_seconds: now,
                artifact_revision,
                cache_version: STACK_PREVIEW_CACHE_VERSION,
                stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
                order: request.order,
                scoring,
                method: Some(method),
                groups: public_groups,
                error: None,
                color_defaults: request.color_defaults.clone(),
                preparing: None,
                replaced_by: Vec::new(),
            },
            groups: prepared_groups,
            stack_root: ctx.stack_root.clone(),
            calibration_root: ctx.calibration_root.clone(),
            north_up: request.north_up,
            order: request.order,
            orientation_peers: Vec::new(),
        },
        identities,
    ))
}

pub(super) fn exclusion_reason(
    image: &AcquiredImage,
    scored: &ImageQualityResult,
    accepted_only: bool,
) -> Option<String> {
    if image.grading_status == 2 {
        Some("Database grade is Rejected".to_string())
    } else if accepted_only && image.grading_status != 1 {
        Some("Accepted-only policy excludes Pending images".to_string())
    } else {
        scored.regrade_reason.clone()
    }
}

pub(super) fn quality_results(
    ctx: &DatabaseContext,
    images: &[(AcquiredImage, String, String)],
    expected_by_image: &HashMap<i32, Option<(f64, f64)>>,
    mapped_sources: &crate::server::remote_upload::MappedLightSources,
    scoring: &StackScoringSettings,
) -> Vec<ImageQualityResult> {
    crate::server::spatial_scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let mut grouped: BTreeMap<(i32, String, String), Vec<&AcquiredImage>> = BTreeMap::new();
    for (image, _, target_name) in images {
        grouped
            .entry((
                image.target_id,
                target_name.clone(),
                image.filter_name.clone(),
            ))
            .or_default()
            .push(image);
    }
    let mut config = SequenceAnalyzerConfig::default();
    scoring.apply_to(&mut config);
    let session_gap = config.session_gap_minutes;
    let analyzer = SequenceAnalyzer::new(config);
    let mut output = Vec::new();
    for ((target_id, target_name, filter_name), group) in grouped {
        let mut metrics = Vec::with_capacity(group.len());
        let mut entries = Vec::with_capacity(group.len());
        for image in group {
            let mut value = extract_light_metrics(image);
            let mapped_source = mapped_sources.get(&image.id);
            let mapped_source_revision = mapped_sources.quality_revision(image.id);
            crate::server::handlers::merge_spatial_metrics(
                &mut value,
                &ctx.spatial_metrics,
                &image.metadata,
                mapped_source_revision,
            );
            crate::server::handlers::merge_astrometry_metrics(
                &mut value,
                &ctx.cache_dir_path,
                &image.metadata,
                &ctx.astrometry_evidence,
                expected_by_image.get(&image.id).copied().flatten(),
                // The planned rotation is display only; the rotation-skew flag
                // comes from the solved rotations alone and still excludes here.
                None,
                mapped_source.map(|source| source.path.as_path()),
                mapped_sources.is_invalid(image.id),
            );
            entries.push(crate::server::handlers::stored_entry_for(
                &ctx.spatial_metrics,
                image.id,
                &image.metadata,
                mapped_source_revision,
            ));
            metrics.push(value);
        }
        crate::server::handlers::merge_photometric_signals(&mut metrics, &entries, session_gap);
        let (sequences, rollup) = analyzer.analyze_with_target_filter_rollup(
            &metrics,
            target_id,
            &target_name,
            &filter_name,
        );
        let session_results = sequences
            .into_iter()
            .flat_map(|sequence| sequence.images)
            .collect();
        output.extend(prefer_target_filter_scores(
            session_results,
            rollup.map(|rollup| rollup.sequence.images),
        ));
    }
    output
}

pub(super) fn prefer_target_filter_scores(
    session_results: Vec<ImageQualityResult>,
    rollup_results: Option<Vec<ImageQualityResult>>,
) -> Vec<ImageQualityResult> {
    let mut rollup_by_id = rollup_results
        .into_iter()
        .flatten()
        .map(|image| (image.image_id, image))
        .collect::<HashMap<_, _>>();
    session_results
        .into_iter()
        .map(|session| rollup_by_id.remove(&session.image_id).unwrap_or(session))
        .collect()
}

pub(super) fn fallback_quality(image_id: i32) -> ImageQualityResult {
    use crate::sequence_analysis::NormalizedMetrics;

    ImageQualityResult {
        image_id,
        quality_score: 1.0,
        temporal_anomaly_score: 0.0,
        category: None,
        flags: Vec::new(),
        normalized_metrics: NormalizedMetrics {
            star_count: None,
            hfr: None,
            eccentricity: None,
            snr: None,
            background: None,
            spatial_coverage: None,
            transparency: None,
            pointing: None,
        },
        regrade_reason: None,
        spatial_overlay: None,
        pointing: None,
        satellite: None,
        details: None,
    }
}
