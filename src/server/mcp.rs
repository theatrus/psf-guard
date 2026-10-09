//! Model Context Protocol server for agents.
//!
//! The endpoint sits inside the API router at `/api/mcp`, so the same
//! middleware that guards the UI guards it: a session cookie, a personal
//! `Authorization: Bearer psfg_…` token, or the open access a server with no
//! accounts gives its own machine. Each tool is a thin call into an existing
//! HTTP handler. The middleware cannot tell a read from a write by method
//! here, because MCP carries everything as a POST, so the tools that change
//! the catalog check the caller's role themselves.

use axum::{
    extract::{Json, Path, Query, State},
    http::request::Parts,
};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    ErrorData as McpError, RoleServer, ServerHandler,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{
    astrobin::AstroBinDetail,
    commands::import::ImportScope,
    server::{
        api::{
            ApiResponse, BatchGradeEntry, BatchGradeRequest, ImageQuery, ImportRequest,
            QualityBackfillRequest, ScoringOverrideQuery, SequenceAnalysisQuery,
        },
        astrobin_export::{self, AstroBinExportQuery},
        auth::{AccessRole, RequestAccess},
        extract::DbContext,
        handlers::{self, AppError},
        sky_coverage,
        state::AppState,
        wbpp_run::{self, StartWbppRunRequest},
    },
};

const DEFAULT_IMAGE_LIMIT: i32 = 100;
const MAX_IMAGE_LIMIT: i32 = 1000;

/// Build the tower service the API router nests at `/mcp`.
pub fn service(state: Arc<AppState>) -> StreamableHttpService<PsfGuardMcp, LocalSessionManager> {
    // The API already answers any Host a proxy forwards, and a network bind
    // needs a login before this endpoint answers, so rmcp's own Host
    // allowlist (loopback names only) would only break reverse proxies.
    // Stateless: every call carries its own credentials, nothing lives
    // between calls, and a reverse proxy needs no sticky sessions. Plain
    // JSON answers suit scripts as well as MCP clients.
    let config = StreamableHttpServerConfig::default()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .disable_allowed_hosts();
    StreamableHttpService::new(
        move || Ok(PsfGuardMcp::new(Arc::clone(&state))),
        LocalSessionManager::default().into(),
        config,
    )
}

#[derive(Clone)]
pub struct PsfGuardMcp {
    state: Arc<AppState>,
}

type ToolResult = Result<CallToolResult, McpError>;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DatabaseArgs {
    /// Database id (slug) or name, as `list_databases` reports it.
    pub database: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ImageListArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Restrict to one project.
    pub project_id: Option<i32>,
    /// Restrict to one target.
    pub target_id: Option<i32>,
    /// Grade filter: `pending`, `accepted`, or `rejected`.
    pub status: Option<String>,
    /// Page size, at most 1000 (default 100).
    pub limit: Option<i32>,
    /// Rows to skip for paging.
    pub offset: Option<i32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ImageArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// The image id from `list_images`.
    pub image_id: i32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ProjectArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// The project id from `list_projects`.
    pub project_id: i32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SequenceArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Score one target's sequence.
    pub target_id: Option<i32>,
    /// Score every target in one project.
    pub project_id: Option<i32>,
    /// Score every target in the database.
    #[serde(default)]
    pub all_projects: bool,
    /// Only this filter's frames.
    pub filter_name: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GradeEntry {
    pub image_id: i32,
    /// `accepted`, `rejected`, or `pending`.
    pub status: String,
    /// Why, for a rejection. Stored as the reject reason.
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GradeArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// The grades to set. Entries may mix statuses.
    pub updates: Vec<GradeEntry>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BackfillArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Recompute evidence that is already cached.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ImportArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// `all` (default), `lights`, or `calibration`.
    pub scope: Option<ImportScopeArg>,
    /// Plan and count without writing anything.
    #[serde(default)]
    pub dry_run: bool,
    /// Queue the quality scan for the new frames afterwards.
    pub backfill: Option<bool>,
    /// Take lights from a rig the catalog has not seen before.
    pub accept_other_rigs: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WbppArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Stack one project; give this or `target_id`.
    pub project_id: Option<i32>,
    /// Stack one target.
    pub target_id: Option<i32>,
    /// Include ungraded lights. Rejects never go in.
    #[serde(default)]
    pub include_pending: bool,
    /// Only this filter's frames.
    pub filter_name: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AstroBinArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Export one project; give this or `target_id`.
    pub project_id: Option<i32>,
    /// Export one target.
    pub target_id: Option<i32>,
    /// Count ungraded lights too. Rejects never count.
    #[serde(default)]
    pub include_pending: bool,
    /// `essentials` (default) or `full`.
    pub detail: Option<AstroBinDetailArg>,
}

/// Mirrors [`ImportScope`] so the schema can name the choices.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportScopeArg {
    All,
    Lights,
    Calibration,
}

impl From<ImportScopeArg> for ImportScope {
    fn from(value: ImportScopeArg) -> Self {
        match value {
            ImportScopeArg::All => Self::All,
            ImportScopeArg::Lights => Self::Lights,
            ImportScopeArg::Calibration => Self::Calibration,
        }
    }
}

/// Mirrors [`AstroBinDetail`] so the schema can name the choices.
#[derive(Debug, Clone, Copy, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AstroBinDetailArg {
    Essentials,
    Full,
}

impl From<AstroBinDetailArg> for AstroBinDetail {
    fn from(value: AstroBinDetailArg) -> Self {
        match value {
            AstroBinDetailArg::Essentials => Self::Essentials,
            AstroBinDetailArg::Full => Self::Full,
        }
    }
}

#[derive(Debug, Serialize)]
struct JobsSnapshot {
    import: crate::server::import_job::ImportJobProgress,
    quality_backfill: crate::server::quality_backfill::QualityBackfillProgress,
    wbpp_run: crate::server::wbpp_run::WbppRunProgress,
}

#[tool_router]
impl PsfGuardMcp {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
    }

    fn access(&self, ctx: &RequestContext<RoleServer>) -> Result<RequestAccess, McpError> {
        if let Some(access) = ctx
            .extensions
            .get::<Parts>()
            .and_then(|parts| parts.extensions.get::<RequestAccess>())
        {
            return Ok(access.clone());
        }
        // No middleware ran (a direct in-process mount). Only a server with no
        // accounts that trusts its callers stays open.
        if self.state.server_auth().is_none() && self.state.anonymous_access_trusted() {
            return Ok(RequestAccess {
                role: AccessRole::ReadWrite,
                username: None,
                api_token: false,
            });
        }
        Err(McpError::invalid_request(
            "Sign in or send an API token to use this server",
            None,
        ))
    }

    fn require_write(&self, ctx: &RequestContext<RoleServer>) -> Result<(), CallToolResult> {
        match self.access(ctx) {
            Ok(access) if access.role == AccessRole::ReadWrite => Ok(()),
            Ok(_) => Err(failure("This account has read-only access")),
            Err(error) => Err(failure(error.message.as_ref())),
        }
    }

    fn database(&self, name: &str) -> Result<DbContext, CallToolResult> {
        let wanted = name.trim();
        let databases = self.state.all_databases();
        databases
            .iter()
            .find(|db| db.id == wanted)
            .or_else(|| {
                databases
                    .iter()
                    .find(|db| db.name.eq_ignore_ascii_case(wanted))
            })
            .cloned()
            .map(DbContext)
            .ok_or_else(|| {
                let known = databases
                    .iter()
                    .map(|db| format!("{} ({})", db.id, db.name))
                    .collect::<Vec<_>>()
                    .join(", ");
                failure(&format!("No database '{wanted}'. Known: {known}"))
            })
    }

    #[tool(
        description = "List the catalogs this server has open: id (slug), name, database path, and image folders. Every other tool takes one of these ids."
    )]
    async fn list_databases(&self) -> ToolResult {
        Ok(render(
            handlers::list_databases(State(Arc::clone(&self.state))).await,
        ))
    }

    #[tool(
        description = "Projects in a database with their targets, exposure plans, progress and recent frames."
    )]
    async fn list_projects(&self, Parameters(args): Parameters<DatabaseArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        Ok(render(handlers::get_projects_overview(ctx).await))
    }

    #[tool(description = "Targets in a database with coordinates, grade counts and last capture.")]
    async fn list_targets(&self, Parameters(args): Parameters<DatabaseArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        Ok(render(handlers::get_targets_overview(ctx).await))
    }

    #[tool(
        description = "Images (light frames) with grade, filter, exposure and stored metrics. Filter by project, target or grade; page with limit and offset."
    )]
    async fn list_images(&self, Parameters(args): Parameters<ImageListArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        let limit = args
            .limit
            .unwrap_or(DEFAULT_IMAGE_LIMIT)
            .clamp(1, MAX_IMAGE_LIMIT);
        let query = ImageQuery {
            project_id: args.project_id,
            target_id: args.target_id,
            status: args.status,
            limit: Some(limit),
            offset: args.offset,
        };
        Ok(render(handlers::get_images(ctx, Query(query)).await))
    }

    #[tool(description = "One image: grade, file location, header metadata and stored metrics.")]
    async fn get_image(&self, Parameters(args): Parameters<ImageArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        let db_id = ctx.id.clone();
        Ok(render(
            handlers::get_image(ctx, Path((db_id, args.image_id))).await,
        ))
    }

    #[tool(
        description = "Quality context for one image: its score, the issues found, and how it sits in its target and filter sequence. Uses stored evidence; it does not start a scan."
    )]
    async fn get_image_quality(&self, Parameters(args): Parameters<ImageArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        let db_id = ctx.id.clone();
        Ok(render(
            handlers::get_image_quality(
                ctx,
                Path((db_id, args.image_id)),
                Query(ScoringOverrideQuery::default()),
            )
            .await,
        ))
    }

    #[tool(
        description = "Score a sequence of frames relative to each other: per-image scores, suggested rejects and their reasons, for one target, one project, or the whole database. Suggestions are advice; grade_images applies them."
    )]
    async fn analyze_sequence(&self, Parameters(args): Parameters<SequenceArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        let query = SequenceAnalysisQuery {
            target_id: args.target_id,
            project_id: args.project_id,
            all_projects: args.all_projects,
            filter_name: args.filter_name,
            ..SequenceAnalysisQuery::default()
        };
        Ok(render(handlers::analyze_sequence(ctx, Query(query)).await))
    }

    #[tool(description = "Whole-database counts: images by grade, projects, targets, exposure.")]
    async fn get_statistics(&self, Parameters(args): Parameters<DatabaseArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        Ok(render(handlers::get_overall_stats(ctx).await))
    }

    #[tool(
        description = "Which darks, flats and bias frames the calibration library can match to a project's lights, and which nights lack them."
    )]
    async fn get_calibration_report(
        &self,
        Parameters(args): Parameters<ProjectArgs>,
    ) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        let db_id = ctx.id.clone();
        Ok(render(
            handlers::get_project_calibration_report(ctx, Path((db_id, args.project_id))).await,
        ))
    }

    #[tool(
        description = "Sky coverage: every target's footprint and exposure by filter, for planning."
    )]
    async fn get_sky_coverage(&self, Parameters(args): Parameters<DatabaseArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        Ok(render(sky_coverage::get_sky_coverage(ctx).await))
    }

    #[tool(
        description = "Progress of the database's background jobs: import, quality scan, and WBPP run. Poll this after starting one."
    )]
    async fn get_jobs(&self, Parameters(args): Parameters<DatabaseArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        let snapshot = JobsSnapshot {
            import: crate::server::import_job::progress_snapshot(&ctx.import_job),
            quality_backfill: crate::server::quality_backfill::snapshot(&ctx.quality_backfill),
            wbpp_run: wbpp_run::progress_snapshot(&ctx.0.wbpp_run),
        };
        Ok(render_value(&snapshot))
    }

    #[tool(
        description = "AstroBin acquisition CSV for a project or target: one row per night and filter, plus the filters that still need an AstroBin id."
    )]
    async fn astrobin_csv(&self, Parameters(args): Parameters<AstroBinArgs>) -> ToolResult {
        let ctx = match self.database(&args.database) {
            Ok(ctx) => ctx,
            Err(error) => return Ok(error),
        };
        let query = AstroBinExportQuery {
            project_id: args.project_id,
            target_id: args.target_id,
            include_pending: args.include_pending,
            detail: args.detail.map(Into::into).unwrap_or_default(),
        };
        Ok(render(
            astrobin_export::get_astrobin_export(State(Arc::clone(&self.state)), ctx, Query(query))
                .await,
        ))
    }

    #[tool(
        description = "Set grades on images (accepted, rejected or pending) with an optional reason. Needs write access. Returns the grades it replaced so the change can be undone."
    )]
    async fn grade_images(
        &self,
        Parameters(args): Parameters<GradeArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        if let Err(denied) = self.require_write(&ctx) {
            return Ok(denied);
        }
        let db = match self.database(&args.database) {
            Ok(db) => db,
            Err(error) => return Ok(error),
        };
        let request = BatchGradeRequest {
            updates: args
                .updates
                .into_iter()
                .map(|entry| BatchGradeEntry {
                    image_id: entry.image_id,
                    status: entry.status,
                    reason: entry.reason,
                })
                .collect(),
        };
        Ok(render(
            handlers::batch_update_image_grades(State(Arc::clone(&self.state)), db, Json(request))
                .await,
        ))
    }

    #[tool(
        description = "Start the background quality scan (stars, background, photometry, pointing) for a database. Needs write access. Follow it with get_jobs."
    )]
    async fn start_quality_backfill(
        &self,
        Parameters(args): Parameters<BackfillArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        if let Err(denied) = self.require_write(&ctx) {
            return Ok(denied);
        }
        let db = match self.database(&args.database) {
            Ok(db) => db,
            Err(error) => return Ok(error),
        };
        let request = QualityBackfillRequest {
            force: args.force,
            fill_metadata: None,
        };
        Ok(render(
            handlers::start_quality_backfill_route(
                State(Arc::clone(&self.state)),
                db,
                Json(request),
            )
            .await,
        ))
    }

    #[tool(
        description = "Scan the database's configured image folders and import new lights and calibration frames. Needs write access. Follow it with get_jobs."
    )]
    async fn start_import(
        &self,
        Parameters(args): Parameters<ImportArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        if let Err(denied) = self.require_write(&ctx) {
            return Ok(denied);
        }
        let db = match self.database(&args.database) {
            Ok(db) => db,
            Err(error) => return Ok(error),
        };
        let request = ImportRequest {
            image_dirs: None,
            time_gap_days: None,
            profile_id: None,
            dry_run: args.dry_run,
            backfill: args.backfill,
            fill_metadata: None,
            attach_existing: None,
            scope: args.scope.map(Into::into),
            skip_processed: None,
            accept_other_rigs: args.accept_other_rigs,
            match_radius_deg: None,
        };
        Ok(render(
            handlers::start_import_route(State(Arc::clone(&self.state)), db, Json(request)).await,
        ))
    }

    #[tool(
        description = "Run PixInsight WBPP on a project's or target's accepted lights with the server's default WBPP settings. Needs write access and a server started with database management. Follow it with get_jobs."
    )]
    async fn start_wbpp_run(
        &self,
        Parameters(args): Parameters<WbppArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        if let Err(denied) = self.require_write(&ctx) {
            return Ok(denied);
        }
        let db = match self.database(&args.database) {
            Ok(db) => db,
            Err(error) => return Ok(error),
        };
        let request = StartWbppRunRequest {
            project_id: args.project_id,
            target_id: args.target_id,
            include_pending: args.include_pending,
            filter_name: args.filter_name,
            ..StartWbppRunRequest::default()
        };
        Ok(render(
            wbpp_run::start_wbpp_run(State(Arc::clone(&self.state)), db, Json(request)).await,
        ))
    }

    #[tool(description = "Stop the database's running WBPP run. Needs write access.")]
    async fn cancel_wbpp_run(
        &self,
        Parameters(args): Parameters<DatabaseArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        if let Err(denied) = self.require_write(&ctx) {
            return Ok(denied);
        }
        let db = match self.database(&args.database) {
            Ok(db) => db,
            Err(error) => return Ok(error),
        };
        Ok(render(
            wbpp_run::cancel_wbpp_run(State(Arc::clone(&self.state)), db).await,
        ))
    }
}

#[tool_handler]
impl ServerHandler for PsfGuardMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("psf-guard", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "PSF Guard catalogs, grades and stacks astrophotography frames. Start with \
                 list_databases; every other tool takes a database id from it. Grades are \
                 accepted, rejected or pending. analyze_sequence and get_image_quality report \
                 evidence and suggestions; nothing changes until grade_images runs. Jobs \
                 (import, quality scan, WBPP) return at once; poll get_jobs for progress. \
                 Catalog predictions and header values are not pixel evidence; say which one \
                 a conclusion rests on."
                    .to_string(),
            )
    }
}

fn failure(message: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.to_string())])
}

fn render<T: Serialize>(response: Result<Json<ApiResponse<T>>, AppError>) -> CallToolResult {
    match response {
        Ok(Json(body)) => {
            if !body.success {
                return failure(body.error.as_deref().unwrap_or("The request failed"));
            }
            match body.data {
                Some(data) => render_value(&data),
                None => failure(
                    "The catalog cache is still loading for this database; try again shortly",
                ),
            }
        }
        Err(error) => failure(&error_text(&error)),
    }
}

fn render_value<T: Serialize>(value: &T) -> CallToolResult {
    match serde_json::to_string_pretty(value) {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(error) => failure(&format!("Could not serialize the result: {error}")),
    }
}

fn error_text(error: &AppError) -> String {
    match error {
        AppError::NotFound => "Resource not found".to_string(),
        AppError::NotFoundMessage(message)
        | AppError::BadRequest(message)
        | AppError::Conflict(message)
        | AppError::Forbidden(message)
        | AppError::InternalError(message) => message.clone(),
        AppError::DatabaseError(message) => format!("Database error: {message}"),
        AppError::NotImplemented => "Not implemented".to_string(),
    }
}
