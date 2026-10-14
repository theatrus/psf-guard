//! The calibration browser's routes: frames a page at a time, the sessions
//! they were shot in, one frame against its session, a picture of any frame,
//! and the library's masters with a picture of each.
//!
//! Pictures go through the shared preview queue. A request that finds no
//! picture queues one and answers 202; the browser then polls
//! [`post_generation_status`] until it is ready. Pictures are kept below the
//! database's cache folder, keyed by the frame's file as it is now, what is
//! shown, its size and stretch, and [`render::RENDER_VERSION`]. A flat
//! shown against its session also keys on every flat of the session.

pub mod render;

use axum::{
    extract::{Path, Query, State},
    http::HeaderMap,
    response::Response,
    Json,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::calibration::browse::{
    self, BrowsedFrame, FrameDetail, FrameFilter, FramePage, FrameSort, GradeFilter,
    RecordedLibraryMaster, SessionPage, MIN_COMPARE_FLATS,
};
use crate::calibration::{CalibrationFrameSummary, CalibrationKind, FrameSpan};
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::{
    generating_response, serve_cached_png, AppError, GenerationStatusBatch,
};
use crate::server::preview_queue::{GenJob, GenKind, GenerationState, GenerationStatus};
use crate::server::stack_preview::calibration_masters::{
    self as stack_masters, DisplayOptions, MasterUsage,
};
use crate::server::state::AppState;
use crate::server::storage::CALIBRATION_PREVIEWS;
use render::{ExtraOutput, FrameSize, FrameStretch, FrameView, RenderSpec, SessionSource};

/// Preview requests one status poll may carry.
const MAX_STATUS_BATCH: usize = 100;

/// The filters, sort and page of a frame or session list.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListQuery {
    pub rig: Option<String>,
    pub kind: Option<CalibrationKind>,
    pub filter: Option<String>,
    pub exposure: Option<f64>,
    pub temp_min: Option<f64>,
    pub temp_max: Option<f64>,
    pub night_from: Option<String>,
    pub night_to: Option<String>,
    pub grade: Option<GradeFilter>,
    pub kept_out: Option<bool>,
    /// Keep only frames whose file is not on disk. Each file is checked, so
    /// this is slow on a large library over a network share.
    pub missing: Option<bool>,
    pub session: Option<String>,
    pub sort: Option<FrameSort>,
    pub page: Option<usize>,
    pub per_page: Option<usize>,
}

impl ListQuery {
    fn filter(&self) -> FrameFilter {
        FrameFilter {
            rig: self.rig.clone().filter(|rig| !rig.is_empty()),
            kind: self.kind,
            filter: self.filter.clone().filter(|filter| !filter.is_empty()),
            exposure: self.exposure,
            temp_min: self.temp_min,
            temp_max: self.temp_max,
            night_from: self.night_from.clone().filter(|night| !night.is_empty()),
            night_to: self.night_to.clone().filter(|night| !night.is_empty()),
            grade: self.grade,
            kept_out: self.kept_out,
            session: self.session.clone().filter(|session| !session.is_empty()),
        }
    }
}

fn read_library(ctx: &DatabaseContext) -> Result<browse::LibraryFrames, AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    browse::library_frames(&conn).map_err(AppError::db)
}

fn joined(error: tokio::task::JoinError) -> AppError {
    AppError::InternalError(format!("calibration browser task failed: {error}"))
}

/// The folders a recorded frame is read from: the database's image folders
/// and its calibration folder. A catalog synced from another machine can
/// record any path; a frame outside these folders reads as a missing file,
/// so no request opens or reports on a path elsewhere.
pub(crate) struct ReadableRoots(Vec<PathBuf>);

impl ReadableRoots {
    pub(crate) fn of(ctx: &DatabaseContext) -> Self {
        Self(
            ctx.image_dir_paths
                .iter()
                .chain(std::iter::once(&ctx.calibration_root))
                .filter_map(|root| dunce::canonicalize(root).ok())
                .collect(),
        )
    }

    /// Whether `path` is a file inside one of the folders once links are
    /// resolved.
    pub(crate) fn holds(&self, path: &std::path::Path) -> bool {
        dunce::canonicalize(path).is_ok_and(|resolved| {
            resolved.is_file() && self.0.iter().any(|root| resolved.starts_with(root))
        })
    }
}

/// The frames among `rows` whose file is not on disk.
fn missing_files<'a>(
    rows: impl Iterator<Item = &'a BrowsedFrame>,
    roots: &ReadableRoots,
) -> HashSet<String> {
    rows.filter(|row| !roots.holds(std::path::Path::new(&row.frame.source_path)))
        .map(|row| row.frame.frame_uuid.clone())
        .collect()
}

/// GET /api/db/{db}/calibrations/frames — one page of the frames the
/// filters keep, with whether each file is on disk.
pub async fn list_frames(
    ctx: DbContext,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApiResponse<FramePage>>, AppError> {
    // Read under the shared connection; files are checked after it is free.
    let library = read_library(&ctx)?;
    let roots = ReadableRoots::of(&ctx);
    let page = tokio::task::spawn_blocking(move || {
        let filter = query.filter();
        let missing = query
            .missing
            .unwrap_or(false)
            .then(|| missing_files(library.kept(&filter), &roots));
        let mut page = library.frames_page(
            &filter,
            |row| {
                missing
                    .as_ref()
                    .is_none_or(|missing| missing.contains(&row.frame.frame_uuid))
            },
            query.sort.unwrap_or_default(),
            query.page.unwrap_or(0),
            query.per_page,
        );
        for row in &mut page.frames {
            row.frame.source_exists = roots.holds(std::path::Path::new(&row.frame.source_path));
        }
        page
    })
    .await
    .map_err(joined)?;
    Ok(Json(ApiResponse::success(page)))
}

/// GET /api/db/{db}/calibrations/sessions — one page of the sessions that
/// hold frames the filters keep, newest night first, with each session's
/// counts by grade and the values the filters can take.
pub async fn list_sessions(
    ctx: DbContext,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApiResponse<SessionPage>>, AppError> {
    let library = read_library(&ctx)?;
    let roots = ReadableRoots::of(&ctx);
    let page = tokio::task::spawn_blocking(move || {
        let filter = query.filter();
        let missing = query
            .missing
            .unwrap_or(false)
            .then(|| missing_files(library.kept(&filter), &roots));
        library.sessions_page(
            &filter,
            |row| {
                missing
                    .as_ref()
                    .is_none_or(|missing| missing.contains(&row.frame.frame_uuid))
            },
            query.page.unwrap_or(0),
            query.per_page,
        )
    })
    .await
    .map_err(joined)?;
    Ok(Json(ApiResponse::success(page)))
}

/// GET /api/db/{db}/calibrations/frames/{frame_uuid} — one frame, its
/// measurement and findings, and its session's numbers beside its own.
pub async fn get_frame(
    ctx: DbContext,
    Path((_db_id, frame_uuid)): Path<(String, String)>,
) -> Result<Json<ApiResponse<FrameDetail>>, AppError> {
    let detail = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        browse::frame_detail(&conn, &frame_uuid).map_err(AppError::db)?
    }
    .ok_or(AppError::NotFound)?;
    let roots = ReadableRoots::of(&ctx);
    let detail = tokio::task::spawn_blocking(move || {
        let mut detail = detail;
        detail.frame.frame.source_exists =
            roots.holds(std::path::Path::new(&detail.frame.frame.source_path));
        detail
    })
    .await
    .map_err(joined)?;
    Ok(Json(ApiResponse::success(detail)))
}

/// What a frame picture shows.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct FramePreviewQuery {
    pub view: Option<FrameView>,
    pub size: Option<FrameSize>,
    pub stretch: Option<FrameStretch>,
}

#[derive(Debug, Clone, Copy)]
struct FramePicture {
    view: FrameView,
    size: FrameSize,
    stretch: FrameStretch,
}

impl FramePreviewQuery {
    fn picture(self) -> FramePicture {
        FramePicture {
            view: self.view.unwrap_or_default(),
            size: self.size.unwrap_or_default(),
            stretch: self.stretch.unwrap_or_default(),
        }
    }
}

/// A frame to draw and, for a comparison, its session's flats.
#[derive(Debug, Clone)]
struct FrameToDraw {
    frame: CalibrationFrameSummary,
    session: Option<Vec<PathBuf>>,
}

/// Find a frame, and its session's flats when it is to be compared. A
/// picture of the frame alone needs one row; only a comparison reads its
/// session.
fn frame_to_draw(
    conn: &Connection,
    frame_uuid: &str,
    view: FrameView,
) -> Result<FrameToDraw, AppError> {
    match view {
        FrameView::Frame => Ok(FrameToDraw {
            frame: browse::frame_row(conn, frame_uuid)
                .map_err(AppError::db)?
                .ok_or(AppError::NotFound)?,
            session: None,
        }),
        FrameView::Session => {
            let (frame, members) = browse::frame_with_session(conn, frame_uuid)
                .map_err(AppError::db)?
                .ok_or(AppError::NotFound)?;
            if frame.frame.kind != CalibrationKind::Flat || frame.frame.is_master {
                return Err(AppError::BadRequest(
                    "Only a raw flat can be compared with its session".into(),
                ));
            }
            let members: Vec<PathBuf> = members
                .into_iter()
                .filter(|member| !member.frame.is_master)
                .map(|member| PathBuf::from(member.frame.source_path))
                .collect();
            if members.len() < MIN_COMPARE_FLATS {
                return Err(AppError::BadRequest(format!(
                    "A flat needs {MIN_COMPARE_FLATS} flats in its session to be compared"
                )));
            }
            Ok(FrameToDraw {
                frame: frame.frame,
                session: Some(members),
            })
        }
    }
}

fn digest(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The queued job that draws one frame picture. Reads file metadata, so
/// call it off the async workers.
fn frame_job(
    ctx: &DatabaseContext,
    drawn: &FrameToDraw,
    picture: FramePicture,
) -> Result<GenJob, AppError> {
    let roots = ReadableRoots::of(ctx);
    let source = PathBuf::from(&drawn.frame.source_path);
    if !roots.holds(&source) {
        return Err(AppError::NotFoundMessage(
            "The frame's file is not on disk".into(),
        ));
    }
    let members = drawn.session.as_ref().map(|members| {
        members
            .iter()
            .filter(|member| roots.holds(member))
            .cloned()
            .collect::<Vec<_>>()
    });
    if members
        .as_ref()
        .is_some_and(|members| members.len() < MIN_COMPARE_FLATS)
    {
        return Err(AppError::BadRequest(format!(
            "A flat needs {MIN_COMPARE_FLATS} flats in its session to be compared"
        )));
    }
    let folder = ctx
        .cache()
        .ensure_category_dir(CALIBRATION_PREVIEWS)
        .map_err(|error| {
            AppError::InternalError(format!("Could not create the preview folder: {error}"))
        })?;
    let session = members.as_ref().map(|members| {
        let mut parts: Vec<String> = members
            .iter()
            .map(|member| crate::server::stack_preview::source_fingerprint(member))
            .collect();
        parts.sort();
        let key = digest(&format!(
            "{}:median:{}",
            render::RENDER_VERSION,
            parts.join(",")
        ));
        (
            key.clone(),
            SessionSource {
                members: members.clone(),
                median_path: folder.join(format!("{key}.median")),
            },
        )
    });
    let fingerprint = crate::server::stack_preview::source_fingerprint(&source);
    let kind = drawn.frame.kind;
    let path_for = |view: FrameView, size: FrameSize| {
        folder.join(format!(
            "{}.png",
            digest(&format!(
                "{}:{}:{fingerprint}:{}:{}:{}:{}:{}",
                render::RENDER_VERSION,
                drawn.frame.frame_uuid,
                kind.as_str(),
                view.as_str(),
                size.as_str(),
                picture.stretch.as_str(),
                session.as_ref().map(|(key, _)| key.as_str()).unwrap_or(""),
            ))
        ))
    };
    // Reading a frame costs far more than drawing it, so the strip's
    // picture and the viewer's come from one read.
    let also = match (picture.view, picture.size) {
        (FrameView::Frame, FrameSize::Thumb) => Some(FrameSize::Screen),
        (FrameView::Frame, FrameSize::Screen) => Some(FrameSize::Thumb),
        _ => None,
    }
    .map(|size| ExtraOutput {
        path: path_for(FrameView::Frame, size),
        max_side: size.max_side(),
    })
    .into_iter()
    .collect();
    Ok(GenJob {
        cache_path: path_for(picture.view, picture.size),
        fits_path: source,
        kind: GenKind::CalibrationFrame(Box::new(RenderSpec {
            kind,
            session: session.map(|(_, session)| session),
            stretch: picture.stretch,
            max_side: picture.size.max_side(),
            also,
        })),
        encoding: crate::preview_format::PreviewEncoding::png(),
    })
}

async fn answer_preview(
    status: GenerationStatus,
    cache_path: &std::path::Path,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    match status.state {
        GenerationState::Ready => serve_cached_png(cache_path, headers).await,
        GenerationState::Generating => Ok(generating_response()),
        GenerationState::Error => Err(AppError::InternalError(
            status
                .error
                .unwrap_or_else(|| "The preview could not be drawn".into()),
        )),
    }
}

/// GET /api/db/{db}/calibrations/frames/{frame_uuid}/preview — the frame
/// drawn for its kind, or a flat against its session: the picture when it
/// is ready, 202 while it is queued. Never drawn in the request.
pub async fn get_frame_preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, frame_uuid)): Path<(String, String)>,
    Query(query): Query<FramePreviewQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let picture = query.picture();
    let drawn = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        frame_to_draw(&conn, &frame_uuid, picture.view)?
    };
    let ctx = Arc::clone(&ctx.0);
    let (status, cache_path) = tokio::task::spawn_blocking(move || {
        let job = frame_job(&ctx, &drawn, picture)?;
        let cache_path = job.cache_path.clone();
        Ok::<_, AppError>((stack_masters::generation_status(&state, job), cache_path))
    })
    .await
    .map_err(joined)??;
    answer_preview(status, &cache_path, &headers).await
}

/// One picture a page waits for.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "target", rename_all = "snake_case")]
pub enum StatusItem {
    Frame {
        id: String,
        view: Option<FrameView>,
        size: Option<FrameSize>,
        stretch: Option<FrameStretch>,
    },
    Master {
        id: String,
        size: Option<String>,
        midtone: Option<f64>,
        shadow: Option<f64>,
    },
}

#[derive(Debug, Deserialize)]
pub struct StatusRequest {
    requests: Vec<StatusItem>,
}

/// What a status poll found for one item before touching files.
enum Resolved {
    Frame(Result<Box<FrameToDraw>, AppError>, FramePicture),
    Master(Option<Box<RecordedLibraryMaster>>, DisplayOptions),
}

/// POST /api/db/{db}/calibrations/generation-status — whether each frame
/// or master picture is ready, queueing any that is neither ready nor
/// queued. Parallel to `requests`.
pub async fn post_generation_status(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<StatusRequest>,
) -> Result<Json<ApiResponse<GenerationStatusBatch>>, AppError> {
    if request.requests.len() > MAX_STATUS_BATCH {
        return Err(AppError::BadRequest(
            "Too many calibration preview status requests".into(),
        ));
    }
    let resolved: Vec<Resolved> = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let mut masters: Option<HashMap<String, RecordedLibraryMaster>> = None;
        let mut resolved = Vec::with_capacity(request.requests.len());
        for item in request.requests {
            resolved.push(match item {
                StatusItem::Frame {
                    id,
                    view,
                    size,
                    stretch,
                } => {
                    let picture = FramePreviewQuery {
                        view,
                        size,
                        stretch,
                    }
                    .picture();
                    Resolved::Frame(
                        frame_to_draw(&conn, &id, picture.view).map(Box::new),
                        picture,
                    )
                }
                StatusItem::Master {
                    id,
                    size,
                    midtone,
                    shadow,
                } => {
                    if masters.is_none() {
                        masters = Some(
                            browse::recorded_masters(&conn)
                                .map_err(AppError::db)?
                                .into_iter()
                                .map(|master| (master.master_uuid.clone(), master))
                                .collect(),
                        );
                    }
                    Resolved::Master(
                        masters
                            .as_ref()
                            .and_then(|masters| masters.get(&id).cloned())
                            .map(Box::new),
                        DisplayOptions {
                            size,
                            midtone,
                            shadow,
                        },
                    )
                }
            });
        }
        resolved
    };
    let ctx = Arc::clone(&ctx.0);
    let statuses = tokio::task::spawn_blocking(move || {
        resolved
            .into_iter()
            .map(|item| {
                let job = match item {
                    Resolved::Frame(drawn, picture) => {
                        drawn.and_then(|drawn| frame_job(&ctx, &drawn, picture))
                    }
                    Resolved::Master(master, display) => master
                        .ok_or(AppError::NotFound)
                        .and_then(|master| master_job(&ctx, &master, &display)),
                };
                match job {
                    Ok(job) => stack_masters::generation_status(&state, job),
                    Err(error) => GenerationStatus {
                        state: GenerationState::Error,
                        error: Some(match error {
                            AppError::BadRequest(message) | AppError::NotFoundMessage(message) => {
                                message
                            }
                            _ => "The calibration picture is unavailable".into(),
                        }),
                    },
                }
            })
            .collect()
    })
    .await
    .map_err(joined)?;
    Ok(Json(ApiResponse::success(GenerationStatusBatch {
        statuses,
    })))
}

/// A project whose latest stacks use a master.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectUse {
    pub project_id: i32,
    pub name: Option<String>,
}

/// A master the library recorded, in the shape the master inspector reads,
/// with what the library knows of it.
#[derive(Debug, Clone, Serialize)]
pub struct LibraryMaster {
    pub id: String,
    pub kind: CalibrationKind,
    /// Its file name.
    pub label: String,
    pub available: bool,
    pub unavailable_reason: Option<String>,
    /// Always empty: a library master belongs to no one stack.
    pub usages: Vec<MasterUsage>,
    pub preview_url: Option<String>,
    pub original_preview_url: Option<String>,
    pub fits_url: Option<String>,
    pub source_count: Option<usize>,
    pub rejection_method: Option<String>,
    pub masked_samples: Option<u64>,
    pub rejected_samples: Option<u64>,
    pub minimum_clean_samples: Option<u64>,
    pub maximum_clean_samples: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sources: Option<FrameSpan>,
    pub rig_uuid: String,
    pub rig_name: Option<String>,
    pub created_at: i64,
    pub exposure_s: Option<f64>,
    pub filter: Option<String>,
    pub camera_temp: Option<f64>,
    /// A bias or dark master from fewer frames than the thin limit.
    pub thin: bool,
    /// A master from other software, used as it is.
    pub external: bool,
    pub defect_pixels_replaced: Option<u64>,
    /// Seiza's verdict on a flat master's set, as recorded.
    pub flat_set: Option<serde_json::Value>,
    /// What the flat-set check said, when it said something.
    pub flat_note: Option<String>,
    /// A flat master the flat-set check set aside for another set.
    pub set_aside: bool,
    /// Projects whose latest stacks use it.
    pub used_by: Vec<ProjectUse>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LibraryMasterCatalog {
    pub masters: Vec<LibraryMaster>,
    pub notes: Vec<String>,
}

fn label_of(master: &RecordedLibraryMaster) -> String {
    master
        .cache_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn library_master(
    ctx: &DatabaseContext,
    master: RecordedLibraryMaster,
    used_by: Vec<ProjectUse>,
) -> LibraryMaster {
    let label = label_of(&master);
    let available = stack_masters::master_file(ctx, master.kind, &label).is_ok();
    let base = format!(
        "/api/db/{}/calibrations/masters/{}",
        ctx.id, master.master_uuid
    );
    let statistics = &master.statistics;
    let number = |value: &serde_json::Value| value.as_u64();
    let external = statistics["external"].as_bool().unwrap_or(false);
    let masking = &statistics["flat_star_masking"];
    LibraryMaster {
        id: master.master_uuid.clone(),
        kind: master.kind,
        label,
        available,
        unavailable_reason: (!available).then(|| {
            "The master file is no longer in this database's cache; a stack builds it again".into()
        }),
        usages: Vec::new(),
        preview_url: available.then(|| format!("{base}/preview")),
        original_preview_url: available.then(|| format!("{base}/preview?size=original")),
        fits_url: available.then(|| format!("{base}/fits")),
        source_count: Some(master.source_count),
        rejection_method: statistics["rejection_method"].as_str().map(str::to_owned),
        masked_samples: number(&statistics["masked_samples"]),
        rejected_samples: number(&statistics["rejected_samples"]),
        minimum_clean_samples: number(&masking["minimum_clean_samples"]),
        maximum_clean_samples: number(&masking["maximum_clean_samples"]),
        sources: master.sources,
        rig_uuid: master.rig_uuid,
        rig_name: master.rig_name,
        created_at: master.created_at,
        exposure_s: master.exposure_s,
        filter: master.filter,
        camera_temp: master.camera_temp,
        // A master recorded before thin masters were marked is judged by
        // its count, as a new one would be.
        thin: statistics["thin"].as_bool().unwrap_or(
            matches!(master.kind, CalibrationKind::Bias | CalibrationKind::Dark)
                && !external
                && master.source_count < crate::calibration::THIN_MASTER_FRAMES,
        ),
        external,
        defect_pixels_replaced: number(&statistics["defect_pixels_replaced"]),
        flat_set: statistics
            .get("frame_quality")
            .filter(|value| value.is_object())
            .cloned(),
        flat_note: statistics["flat_stability"]["note"]
            .as_str()
            .map(str::to_owned),
        set_aside: statistics["flat_stability"]["superseded"]
            .as_bool()
            .unwrap_or(false),
        used_by,
    }
}

fn project_names(conn: &Connection) -> HashMap<i32, String> {
    // A catalog with no project table names none.
    conn.prepare("SELECT Id, name FROM project")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<HashMap<i32, String>>>()
        })
        .unwrap_or_default()
}

/// GET /api/db/{db}/calibrations/masters — every master the library
/// recorded, newest first, with its sources, its statistics and the
/// projects whose latest stacks use it.
pub async fn list_masters(
    ctx: DbContext,
) -> Result<Json<ApiResponse<LibraryMasterCatalog>>, AppError> {
    let (masters, names) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        (
            browse::recorded_masters(&conn).map_err(AppError::db)?,
            project_names(&conn),
        )
    };
    let ctx = Arc::clone(&ctx.0);
    let catalog = tokio::task::spawn_blocking(move || {
        let projects = crate::server::master_cleanup::projects_by_label(&ctx.stack_root);
        let masters = masters
            .into_iter()
            .map(|master| {
                let used_by = projects
                    .get(&label_of(&master))
                    .map(|ids| {
                        ids.iter()
                            .map(|id| ProjectUse {
                                project_id: *id,
                                name: names.get(id).cloned(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                library_master(&ctx, master, used_by)
            })
            .collect();
        LibraryMasterCatalog {
            masters,
            notes: Vec::new(),
        }
    })
    .await
    .map_err(joined)?;
    Ok(Json(ApiResponse::success(catalog)))
}

fn recorded_master(
    ctx: &DatabaseContext,
    master_uuid: &str,
) -> Result<RecordedLibraryMaster, AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    browse::recorded_master(&conn, master_uuid)
        .map_err(AppError::db)?
        .ok_or(AppError::NotFound)
}

fn master_job(
    ctx: &DatabaseContext,
    master: &RecordedLibraryMaster,
    display: &DisplayOptions,
) -> Result<GenJob, AppError> {
    let path = stack_masters::master_file(ctx, master.kind, &label_of(master))?;
    stack_masters::master_preview_job(
        ctx,
        &format!("library:{}", master.master_uuid),
        path,
        display,
    )
}

/// GET /api/db/{db}/calibrations/masters/{master_uuid}/preview — a master
/// drawn with the stack inspector's stretch, without a stack: the picture
/// when ready, 202 while it is queued.
pub async fn get_master_preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, master_uuid)): Path<(String, String)>,
    Query(display): Query<DisplayOptions>,
) -> Result<Response, AppError> {
    let master = recorded_master(&ctx, &master_uuid)?;
    let ctx = Arc::clone(&ctx.0);
    let (status, cache_path) = tokio::task::spawn_blocking(move || {
        let job = master_job(&ctx, &master, &display)?;
        let cache_path = job.cache_path.clone();
        Ok::<_, AppError>((stack_masters::generation_status(&state, job), cache_path))
    })
    .await
    .map_err(joined)??;
    match status.state {
        GenerationState::Ready => stack_masters::serve_file(&cache_path, "image/png", None).await,
        _ => answer_preview(status, &cache_path, &HeaderMap::new()).await,
    }
}

/// GET /api/db/{db}/calibrations/masters/{master_uuid}/fits — the master
/// file as it is.
pub async fn download_master_fits(
    ctx: DbContext,
    Path((_db_id, master_uuid)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let master = recorded_master(&ctx, &master_uuid)?;
    let label = label_of(&master);
    let ctx = Arc::clone(&ctx.0);
    let lookup = label.clone();
    let path =
        tokio::task::spawn_blocking(move || stack_masters::master_file(&ctx, master.kind, &lookup))
            .await
            .map_err(joined)??;
    stack_masters::serve_file(&path, "application/fits", Some(&label)).await
}
