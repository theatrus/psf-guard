//! Model Context Protocol server for agents.
//!
//! The endpoint sits inside the API router at `/api/mcp`, behind the same
//! middleware that guards the UI: a session cookie, a personal
//! `Authorization: Bearer psfg_…` token, or the open access a server with no
//! accounts gives its own machine. Each tool sends its request back through
//! that router as the caller (see [`api`]), so the read-only role, database
//! management and every handler's own checks apply to an agent exactly as
//! they do to a person.

use axum::http::Method;
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
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::server::{
    extract::DbContext,
    handlers::AppError,
    sky_coverage,
    stack_preview::{LatestStackPreviews, StackGroupStatus, StackPreviewJob},
    state::AppState,
};
use api::{Caller, Reply};

mod api;
mod stacks;

const DEFAULT_IMAGE_LIMIT: i32 = 100;
const MAX_IMAGE_LIMIT: i32 = 1000;
/// The longest side `get_stack_image` returns by default, and its bounds.
const DEFAULT_STACK_IMAGE_SIZE: u32 = 1024;
const STACK_IMAGE_SIZES: std::ops::RangeInclusive<u32> = 256..=2048;

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
    /// Only this filter's frames, such as `SII`; case does not matter.
    pub filter_name: Option<String>,
    /// Keep only these header keys in each frame's metadata, such as
    /// `["HFR", "DetectedStars", "RotatorPosition", "PierSide"]`. Omitted
    /// keeps every key.
    pub metadata_keys: Option<Vec<String>>,
    /// Page size, at most 1000 (default 100).
    pub limit: Option<i32>,
    /// Rows to skip for paging.
    pub offset: Option<i32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StackArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// The project id from `list_projects`.
    pub project_id: i32,
    /// The stack's job id from `list_stacks`.
    pub job_id: String,
    /// Which channel of that job, from `list_stacks`. Default 0.
    #[serde(default)]
    pub group_index: usize,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StackImageArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// The project id from `list_projects`.
    pub project_id: i32,
    /// The stack's job id from `list_stacks`.
    pub job_id: String,
    /// Which channel of that job, from `list_stacks`. Default 0.
    #[serde(default)]
    pub group_index: usize,
    /// `display` (default), as the app shows it, or `background`, stretched
    /// hard so gradients, vignetting, streaks and calibration patterns show.
    #[serde(default)]
    pub stretch: stacks::ImageStretch,
    /// Show only this part, as fractions of the width and height from the
    /// top left, e.g. `{"x": 0.5, "y": 0, "width": 0.5, "height": 0.5}` for
    /// the top-right quarter.
    pub crop: Option<stacks::CropArgs>,
    /// Longest side of the returned image, 256 to 2048 pixels (default 1024).
    pub max_size: Option<u32>,
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

/// Which frames an import touches.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportScopeArg {
    All,
    Lights,
    Calibration,
}

/// How much an AstroBin export says.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AstroBinDetailArg {
    Essentials,
    Full,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ApiGetArgs {
    /// A route below `/api` from `api_routes`, with its placeholders filled,
    /// such as `/db/c925/images/4796` or `/director/v1/plans`.
    pub path: String,
    /// Query parameters, such as `{"project_id": "1", "limit": "50"}`.
    #[serde(default)]
    pub query: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PlanArgs {
    /// The plan's id (a UUID) from `list_plans`.
    pub plan_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RigArgs {
    /// The rig's id (a UUID) from `list_rigs`.
    pub rig: String,
    /// A plan id, to see the preferences as that plan's project gets them.
    pub plan_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TemplateArgs {
    /// A rig catalog's database id, for the templates in that rig's
    /// database. Omitted gives the shared template library.
    pub database: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SkySearchArgs {
    /// A name or catalog designation, such as `NGC 7635` or `Bubble`.
    pub query: String,
    /// Also ask the online name resolver (Sesame), through the cache.
    #[serde(default)]
    pub online: bool,
    /// At most this many matches (default 10).
    pub limit: Option<usize>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreatePlanArgs {
    /// The new plan's name, usually the target's.
    pub name: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenamePlanArgs {
    /// The plan's id from `list_plans`.
    pub plan_id: String,
    pub name: String,
    /// The plan's `revision` as `list_plans` or `get_plan` showed it; a
    /// rename since then refuses.
    pub expected_revision: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SavePlanArgs {
    /// The plan's id from `list_plans`.
    pub plan_id: String,
    /// The whole plan: the object `get_plan` returns under `plan`, changed
    /// where you mean to. Its `project_id` is `plan_id` and its `revision`
    /// the one you read; a save since then refuses.
    pub plan: Value,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveFramingArgs {
    /// The plan's id from `list_plans`.
    pub plan_id: String,
    /// The whole framing draft: the object `get_framing` returns under
    /// `draft`, changed. Its `project_id` is `plan_id` and its `revision`
    /// the one you read.
    pub framing: Value,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TakeFromSchedulerArgs {
    /// The plan's id from `list_plans`.
    pub plan_id: String,
    /// The rig (UUID from `list_rigs`) whose Target Scheduler database holds
    /// the values to take.
    pub rig: String,
    /// The plan's and framing's revisions as you last read them.
    pub plan_revision: u64,
    pub framing_revision: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FeasibilityArgs {
    /// The plan's id from `list_plans`.
    pub plan_id: String,
    /// How many nights ahead to look (the server's default when omitted).
    pub nights: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ActivationArgs {
    /// The plan's id from `list_plans`.
    pub plan_id: String,
    /// Collaboration visits to include, as the activation preview lists
    /// them; omitted for none.
    #[serde(default)]
    pub collaboration: Vec<Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ApplyActivationArgs {
    /// The plan's id from `list_plans`.
    pub plan_id: String,
    /// The `preview_digest` from `preview_activation`. If anything changed
    /// since that preview, the apply refuses; preview again.
    pub preview_digest: String,
    /// The same collaboration visits the preview had.
    #[serde(default)]
    pub collaboration: Vec<Value>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ApplySchedulingArgs {
    /// The rig's id from `list_rigs`.
    pub rig: String,
    /// The `digest` from `get_rig_scheduling`. If the rig or its database
    /// changed since, the apply refuses; read it again.
    pub digest: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveTemplateArgs {
    /// The template's id (a UUID): one from `list_templates` to change it,
    /// or a new one to add a template.
    pub template_id: String,
    /// The whole template: `id` (the same UUID), `revision` (0 for a new
    /// one, else the one you read), `name`, `filter_name`, `gain`, `offset`,
    /// `bin`, `readout_mode`, `default_exposure_seconds`, `updated_at_ms`.
    pub template: Value,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TemplateIdArgs {
    /// The template's id from `list_templates`.
    pub template_id: String,
    /// The template's `revision` as you read it; a change since refuses.
    pub revision: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SavePreferencesArgs {
    /// `global`, `site` or `rig`.
    pub scope: String,
    /// The site or rig id; for `global`, the `global_id` from
    /// `api_get /director/v1/preferences`.
    pub id: String,
    /// The whole settings object as `api_get
    /// /director/v1/preferences/{scope}/{id}` returns it, changed.
    pub settings: Value,
}

/// A database by id or name, or the tool error that names the known ones.
macro_rules! database {
    ($self:ident, $name:expr) => {
        match $self.database($name) {
            Ok(db) => db,
            Err(error) => return Ok(error),
        }
    };
}

/// The value, or return the tool error.
macro_rules! attempt {
    ($result:expr) => {
        match $result {
            Ok(value) => value,
            Err(error) => return Ok(error),
        }
    };
}

#[tool_router]
impl PsfGuardMcp {
    pub fn new(state: Arc<AppState>) -> Self {
        Self { state }
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

    /// Send a request through the API as the caller and keep its data.
    async fn send(
        &self,
        ctx: &RequestContext<RoleServer>,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, CallToolResult> {
        let reply = api::send(&self.state, &Caller::of(ctx), method, path, body.as_ref())
            .await
            .map_err(|error| failure(&error))?;
        api::data(reply).map_err(|error| failure(&error))
    }

    async fn get(
        &self,
        ctx: &RequestContext<RoleServer>,
        path: &str,
    ) -> Result<Value, CallToolResult> {
        self.send(ctx, Method::GET, path, None).await
    }

    /// A request whose data is the tool's whole answer.
    async fn answer(
        &self,
        ctx: &RequestContext<RoleServer>,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> ToolResult {
        Ok(match self.send(ctx, method, path, body).await {
            Ok(value) => text_result(&value),
            Err(error) => error,
        })
    }

    /// A stack job of this project, with a pointer to `list_stacks` when it
    /// is not one.
    async fn stack_job(
        &self,
        ctx: &RequestContext<RoleServer>,
        db: &str,
        project_id: i32,
        job_id: &str,
    ) -> Result<StackPreviewJob, CallToolResult> {
        let path = format!(
            "/db/{}/projects/{project_id}/stack-previews/{}",
            api::encode(db),
            api::encode(job_id)
        );
        let reply = api::send(&self.state, &Caller::of(ctx), Method::GET, &path, None)
            .await
            .map_err(|error| failure(&error))?;
        if reply.status() == axum::http::StatusCode::NOT_FOUND {
            return Err(failure(&format!(
                "No stack {job_id} in project {project_id}; list_stacks names the current ones"
            )));
        }
        let value = api::data(reply).map_err(|error| failure(&error))?;
        serde_json::from_value(value)
            .map_err(|error| failure(&format!("Could not read the stack job: {error}")))
    }

    // ---------- The whole API, read only ----------

    #[tool(
        description = "Every route api_get can read: its path below /api, with placeholders such as {db} (a database id), {project_id}, {image_id}, {plan_id} (a planning UUID) and {rig} (a rig UUID), and what it answers. Use it to find data no other tool gives."
    )]
    async fn api_routes(&self) -> ToolResult {
        let routes: Vec<Value> = api::READABLE_ROUTES
            .iter()
            .map(|(path, answers)| json!({ "path": path, "answers": answers }))
            .collect();
        Ok(text_result(&json!({ "routes": routes })))
    }

    #[tool(
        description = "Read any route the UI reads, as you: GET below /api, such as /db/c925/flat-history or /director/v1/projects/<uuid>/mosaic. api_routes lists them. Answers JSON only; images come from get_stack_image."
    )]
    async fn api_get(
        &self,
        Parameters(args): Parameters<ApiGetArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        if let Err(error) = api::readable_path(&args.path) {
            return Ok(failure(&error));
        }
        let path = api::with_query(
            &args.path,
            args.query
                .iter()
                .map(|(key, value)| (key.as_str(), Some(value.clone()))),
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    // ---------- Catalogs ----------

    #[tool(
        description = "List the catalogs this server has open: id (slug), name, database path, and image folders. Every other tool takes one of these ids."
    )]
    async fn list_databases(&self, ctx: RequestContext<RoleServer>) -> ToolResult {
        self.answer(&ctx, Method::GET, "/databases", None).await
    }

    #[tool(
        description = "Projects in a database with their targets, exposure plans, progress and recent frames."
    )]
    async fn list_projects(
        &self,
        Parameters(args): Parameters<DatabaseArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/projects/overview", api::encode(&db.id));
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(description = "Targets in a database with coordinates, grade counts and last capture.")]
    async fn list_targets(
        &self,
        Parameters(args): Parameters<DatabaseArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/targets/overview", api::encode(&db.id));
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "A project's Target Scheduler settings, its targets and their exposure plans: priority, minimum altitude, moon avoidance, desired and accepted counts."
    )]
    async fn get_project_scheduler(
        &self,
        Parameters(args): Parameters<ProjectArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!(
            "/db/{}/projects/{}/scheduler",
            api::encode(&db.id),
            args.project_id
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "Images (light frames) with grade, filter, exposure and stored metrics. Filter by project, target, grade or filter name; keep only the metadata keys asked for; page with limit and offset."
    )]
    async fn list_images(
        &self,
        Parameters(args): Parameters<ImageListArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let limit = args
            .limit
            .unwrap_or(DEFAULT_IMAGE_LIMIT)
            .clamp(1, MAX_IMAGE_LIMIT);
        let path = api::with_query(
            &format!("/db/{}/images", api::encode(&db.id)),
            [
                ("project_id", args.project_id.map(|id| id.to_string())),
                ("target_id", args.target_id.map(|id| id.to_string())),
                ("status", args.status),
                ("filter_name", args.filter_name),
                ("limit", Some(limit.to_string())),
                ("offset", args.offset.map(|offset| offset.to_string())),
            ],
        );
        let mut images = attempt!(self.get(&ctx, &path).await);
        if let Some(keys) = args.metadata_keys {
            for image in images.as_array_mut().into_iter().flatten() {
                if let Some(metadata) = image.get_mut("metadata").and_then(Value::as_object_mut) {
                    metadata.retain(|key, _| keys.iter().any(|wanted| wanted == key));
                }
            }
        }
        Ok(text_result(&images))
    }

    #[tool(description = "One image: grade, file location, header metadata and stored metrics.")]
    async fn get_image(
        &self,
        Parameters(args): Parameters<ImageArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/images/{}", api::encode(&db.id), args.image_id);
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "Quality context for one image: its score, the issues found, and how it sits in its target and filter sequence. Uses stored evidence; it does not start a scan."
    )]
    async fn get_image_quality(
        &self,
        Parameters(args): Parameters<ImageArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!(
            "/db/{}/analysis/image/{}",
            api::encode(&db.id),
            args.image_id
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "Score a sequence of frames relative to each other: per-image scores, suggested rejects and their reasons, for one target, one project, or the whole database. Suggestions are advice; grade_images applies them."
    )]
    async fn analyze_sequence(
        &self,
        Parameters(args): Parameters<SequenceArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = api::with_query(
            &format!("/db/{}/analysis/sequence", api::encode(&db.id)),
            [
                ("target_id", args.target_id.map(|id| id.to_string())),
                ("project_id", args.project_id.map(|id| id.to_string())),
                (
                    "all_projects",
                    args.all_projects.then(|| "true".to_string()),
                ),
                ("filter_name", args.filter_name),
            ],
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(description = "Whole-database counts: images by grade, projects, targets, exposure.")]
    async fn get_statistics(
        &self,
        Parameters(args): Parameters<DatabaseArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/stats/overall", api::encode(&db.id));
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "Sky coverage: every target's footprint and exposure by filter, for planning."
    )]
    async fn get_sky_coverage(
        &self,
        Parameters(args): Parameters<DatabaseArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/sky/coverage", api::encode(&db.id));
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "AstroBin acquisition CSV for a project or target: one row per night and filter, plus the filters that still need an AstroBin id."
    )]
    async fn astrobin_csv(
        &self,
        Parameters(args): Parameters<AstroBinArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let detail = args
            .detail
            .and_then(|detail| serde_json::to_value(detail).ok())
            .and_then(|detail| detail.as_str().map(str::to_owned));
        let path = api::with_query(
            &format!("/db/{}/astrobin-export", api::encode(&db.id)),
            [
                ("project_id", args.project_id.map(|id| id.to_string())),
                ("target_id", args.target_id.map(|id| id.to_string())),
                (
                    "include_pending",
                    args.include_pending.then(|| "true".to_string()),
                ),
                ("detail", detail),
            ],
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    // ---------- Calibration ----------

    #[tool(
        description = "Which darks, flats and bias frames the calibration library can match to a project's lights, and which nights lack them."
    )]
    async fn get_calibration_report(
        &self,
        Parameters(args): Parameters<ProjectArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!(
            "/db/{}/projects/{}/calibration-report",
            api::encode(&db.id),
            args.project_id
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "The calibration library by night: bias, darks, dark-flats and flats with their settings, validity marks, and the masters built from them."
    )]
    async fn get_calibration_library(
        &self,
        Parameters(args): Parameters<DatabaseArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/calibrations", api::encode(&db.id));
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "Why one light got the calibration masters it did: its header readings, then for bias, dark and flat every frame the library holds for its camera, grouped by night: the frames a master would take, the ones that match but lose to a nearer set, and the ones refused with the readings that disagree (rotation off, exposure, temperature, a validity mark, too far in time). Flats for other filters and frames from other cameras are only counted."
    )]
    async fn explain_calibration(
        &self,
        Parameters(args): Parameters<ImageArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!(
            "/db/{}/images/{}/calibration",
            api::encode(&db.id),
            args.image_id
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    // ---------- Stacks ----------

    #[tool(
        description = "A project's latest stack previews, one per channel (target, filter and exposure): frames integrated and left out, total exposure, the masters each calibration session applied, how much more depth would help, and the job id and group index the other stack tools take."
    )]
    async fn list_stacks(
        &self,
        Parameters(args): Parameters<ProjectArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!(
            "/db/{}/projects/{}/stack-previews/latest",
            api::encode(&db.id),
            args.project_id
        );
        let latest: LatestStackPreviews = attempt!(serde_json::from_value(attempt!(
            self.get(&ctx, &path).await
        ))
        .map_err(|error| failure(&format!("Could not read the stacks: {error}"))));
        let stacks: Vec<_> = latest
            .groups
            .iter()
            .map(|entry| stacks::summarize(&entry.job_id, entry.created_unix_seconds, &entry.group))
            .collect();
        Ok(text_result(
            &json!({ "project_id": args.project_id, "stacks": stacks }),
        ))
    }

    #[tool(
        description = "One stack channel frame by frame, in capture order: each frame's disposition and reason, where it registered (shift and rotation in reference pixels), weight, noise and normalization. Also, per night, whether the frames were dithered or walked steadily one way, which turns anything fixed to the sensor into streaks (walking noise)."
    )]
    async fn get_stack(
        &self,
        Parameters(args): Parameters<StackArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let job = attempt!(
            self.stack_job(&ctx, &db.id, args.project_id, &args.job_id)
                .await
        );
        let Some(group) = job.groups.get(args.group_index) else {
            return Ok(failure(&format!(
                "This stack has {} channel(s); group_index {} is not one",
                job.groups.len(),
                args.group_index
            )));
        };
        let (acquired, boundary) = match capture_times(&db, group) {
            Ok(times) => times,
            Err(error) => return Ok(failure(&error_text(&error))),
        };
        let rows = stacks::frame_rows(group, &acquired, |at| stacks::night_of(at, boundary));
        Ok(text_result(&json!({
            "stack": stacks::summarize(&job.job_id, job.created_unix_seconds, group),
            "drift_by_night": stacks::drift_by_night(&rows),
            "frames": rows,
        })))
    }

    #[tool(
        description = "Look at a stack: its preview image, as the app shows it or with the background stretched hard to show gradients, vignetting, streaks and calibration patterns. Crop to a region and choose the size. Returns a PNG image and a note of what part it shows."
    )]
    async fn get_stack_image(
        &self,
        Parameters(args): Parameters<StackImageArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        // The job is read first so a stack of another project is refused.
        attempt!(
            self.stack_job(&ctx, &db.id, args.project_id, &args.job_id)
                .await
        );
        let path = format!(
            "/db/{}/stack-previews/{}/{}/preview",
            api::encode(&db.id),
            api::encode(&args.job_id),
            args.group_index
        );
        let reply = attempt!(
            api::send(&self.state, &Caller::of(&ctx), Method::GET, &path, None)
                .await
                .map_err(|error| failure(&error))
        );
        let bytes = match reply {
            Reply::Bytes { status, body, .. } if status.is_success() => body,
            Reply::Bytes { status, .. } | Reply::Json(status, _)
                if status == axum::http::StatusCode::NOT_FOUND =>
            {
                return Ok(failure(
                    "This stack has no image yet: it is still building, was skipped, or failed",
                ))
            }
            other => {
                return Ok(failure(&api::data(other).err().unwrap_or_else(|| {
                    "The stack image is not a picture".to_string()
                })))
            }
        };
        let max_size = args
            .max_size
            .unwrap_or(DEFAULT_STACK_IMAGE_SIZE)
            .clamp(*STACK_IMAGE_SIZES.start(), *STACK_IMAGE_SIZES.end());
        let (crop, stretch) = (args.crop, args.stretch);
        let rendered = tokio::task::spawn_blocking(move || {
            stacks::render_png(&bytes, crop, max_size, stretch)
        })
        .await;
        let (png, note) = match rendered {
            Ok(Ok(rendered)) => rendered,
            Ok(Err(error)) => return Ok(failure(&error)),
            Err(error) => {
                return Ok(failure(&format!(
                    "Rendering the stack image failed: {error}"
                )))
            }
        };
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&png);
        let note = serde_json::to_string(&note).unwrap_or_default();
        Ok(CallToolResult::success(vec![
            ContentBlock::image(encoded, "image/png"),
            ContentBlock::text(format!(
                "The stack's preview, {note}. `crop` is [x, y, width, height] in the \
                 preview's pixels; the preview is the stack scaled down, in the orientation the \
                 app shows."
            )),
        ]))
    }

    #[tool(
        description = "The calibration masters a stack applied, per session: each bias, dark and flat master, how many lights it calibrated, how many frames it was built from and how outliers were rejected, and notes on sessions that had none. A master built from two frames, or a session with no flat, explains many stack patterns."
    )]
    async fn get_stack_calibration(
        &self,
        Parameters(args): Parameters<StackArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let job = attempt!(
            self.stack_job(&ctx, &db.id, args.project_id, &args.job_id)
                .await
        );
        let path = api::with_query(
            &format!(
                "/db/{}/stack-previews/{}/{}/calibration-masters",
                api::encode(&db.id),
                api::encode(&job.job_id),
                args.group_index
            ),
            [("revision", Some(job.artifact_revision.clone()))],
        );
        let mut catalog = attempt!(self.get(&ctx, &path).await);
        // The links are for the app's inspector; an agent cannot follow them.
        strip_links(&mut catalog);
        Ok(text_result(&catalog))
    }

    // ---------- Jobs ----------

    #[tool(
        description = "Progress of the database's background jobs: import, quality scan, and WBPP run. Poll this after starting one."
    )]
    async fn get_jobs(
        &self,
        Parameters(args): Parameters<DatabaseArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let base = format!("/db/{}", api::encode(&db.id));
        let import = attempt!(self.get(&ctx, &format!("{base}/import")).await);
        let quality = attempt!(
            self.get(&ctx, &format!("{base}/analysis/quality-backfill"))
                .await
        );
        let wbpp = attempt!(self.get(&ctx, &format!("{base}/wbpp/runs/current")).await);
        Ok(text_result(&json!({
            "import": import,
            "quality_backfill": quality,
            "wbpp_run": wbpp,
        })))
    }

    #[tool(
        description = "Stack builds and WBPP runs across every database: what runs now and what waits in the queue."
    )]
    async fn get_activity(&self, ctx: RequestContext<RoleServer>) -> ToolResult {
        let stacks = attempt!(self.get(&ctx, "/stack-activity").await);
        let wbpp = attempt!(self.get(&ctx, "/wbpp/activity").await);
        Ok(text_result(&json!({ "stacks": stacks, "wbpp": wbpp })))
    }

    #[tool(
        description = "Set grades on images (accepted, rejected or pending) with an optional reason. Needs write access. Returns the grades it replaced so the change can be undone."
    )]
    async fn grade_images(
        &self,
        Parameters(args): Parameters<GradeArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let updates: Vec<Value> = args
            .updates
            .into_iter()
            .map(|entry| {
                json!({ "image_id": entry.image_id, "status": entry.status, "reason": entry.reason })
            })
            .collect();
        let path = format!("/db/{}/images/grade", api::encode(&db.id));
        self.answer(
            &ctx,
            Method::POST,
            &path,
            Some(json!({ "updates": updates })),
        )
        .await
    }

    #[tool(
        description = "Start the background quality scan (stars, background, photometry, pointing) for a database. Needs write access. Follow it with get_jobs."
    )]
    async fn start_quality_backfill(
        &self,
        Parameters(args): Parameters<BackfillArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/analysis/quality-backfill", api::encode(&db.id));
        self.answer(
            &ctx,
            Method::POST,
            &path,
            Some(json!({ "force": args.force })),
        )
        .await
    }

    #[tool(
        description = "Scan the database's configured image folders and import new lights and calibration frames. Needs write access. Follow it with get_jobs."
    )]
    async fn start_import(
        &self,
        Parameters(args): Parameters<ImportArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/import", api::encode(&db.id));
        let body = json!({
            "dry_run": args.dry_run,
            "backfill": args.backfill,
            "scope": args.scope,
            "accept_other_rigs": args.accept_other_rigs,
        });
        self.answer(&ctx, Method::POST, &path, Some(body)).await
    }

    #[tool(
        description = "Run PixInsight WBPP on a project's or target's accepted lights with the server's default WBPP settings. Needs write access and a server started with database management. Follow it with get_jobs."
    )]
    async fn start_wbpp_run(
        &self,
        Parameters(args): Parameters<WbppArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/wbpp/runs", api::encode(&db.id));
        let body = json!({
            "project_id": args.project_id,
            "target_id": args.target_id,
            "include_pending": args.include_pending,
            "filter_name": args.filter_name,
        });
        self.answer(&ctx, Method::POST, &path, Some(body)).await
    }

    #[tool(description = "Stop the database's running WBPP run. Needs write access.")]
    async fn cancel_wbpp_run(
        &self,
        Parameters(args): Parameters<DatabaseArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let db = database!(self, &args.database);
        let path = format!("/db/{}/wbpp/runs/current", api::encode(&db.id));
        self.answer(&ctx, Method::DELETE, &path, None).await
    }

    // ---------- Planning ----------

    #[tool(
        description = "Planning: every plan across rigs, with its target, rigs, goals and state. The plan_id other planning tools take comes from here."
    )]
    async fn list_plans(&self, ctx: RequestContext<RoleServer>) -> ToolResult {
        self.answer(&ctx, Method::GET, "/director/v1/plans", None)
            .await
    }

    #[tool(
        description = "One plan: its target and framing, the rigs it uses, each rig's exposures per band (template, exposure, goal) and its settings."
    )]
    async fn get_plan(
        &self,
        Parameters(args): Parameters<PlanArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!("/director/v1/projects/{}/plan", api::encode(&args.plan_id));
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "A plan's progress: per rig and objective, the frames accepted against the goal, taken, rejected, and whether each part is done, active, off or not yet activated."
    )]
    async fn get_plan_progress(
        &self,
        Parameters(args): Parameters<PlanArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/projects/{}/plan/progress",
            api::encode(&args.plan_id)
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "A plan's framing draft: centre, rotation, panels for a mosaic, and the rig each panel uses."
    )]
    async fn get_framing(
        &self,
        Parameters(args): Parameters<PlanArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/projects/{}/framing",
            api::encode(&args.plan_id)
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "A plan's activation: what was last written to the rigs' Target Scheduler databases, and whether the plan has changed since."
    )]
    async fn get_activation(
        &self,
        Parameters(args): Parameters<PlanArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let base = format!(
            "/director/v1/projects/{}/activation",
            api::encode(&args.plan_id)
        );
        let last = attempt!(self.get(&ctx, &base).await);
        let check = attempt!(self.get(&ctx, &format!("{base}/check")).await);
        Ok(text_result(&json!({ "last": last, "check": check })))
    }

    #[tool(
        description = "Rigs: each rig's profile (camera, scope, site, catalog) and its live status as last reported (phase, target, time). The rig id other tools take comes from here."
    )]
    async fn list_rigs(&self, ctx: RequestContext<RoleServer>) -> ToolResult {
        let profiles = attempt!(self.get(&ctx, "/director/v1/rigs/profiles").await);
        let status = attempt!(self.get(&ctx, "/director/v1/rigs/status").await);
        Ok(text_result(
            &json!({ "profiles": profiles, "status": status }),
        ))
    }

    #[tool(
        description = "A rig's effective observing preferences, after global, site, rig and (with plan_id) project settings: altitude, Moon, twilight and the like, with where each value comes from."
    )]
    async fn get_rig_preferences(
        &self,
        Parameters(args): Parameters<RigArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = api::with_query(
            &format!("/director/v1/rigs/{}/preferences", api::encode(&args.rig)),
            [("project_id", args.plan_id)],
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "The Target Scheduler limits a rig's settings would write to each project in its database, and which differ from what the database holds now. Reading it changes nothing."
    )]
    async fn get_rig_scheduling(
        &self,
        Parameters(args): Parameters<RigArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!("/director/v1/rigs/{}/scheduling", api::encode(&args.rig));
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "Exposure templates: the shared library, or with database a rig catalog's own templates (filter, gain, offset, binning, default exposure, Moon settings)."
    )]
    async fn list_templates(
        &self,
        Parameters(args): Parameters<TemplateArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = match args.database {
            Some(name) => {
                let db = database!(self, &name);
                format!("/director/v1/catalogs/{}/templates", api::encode(&db.id))
            }
            None => "/director/v1/templates".to_string(),
        };
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "Find a target by name or designation in the local catalogs (and with online, the Sesame resolver through its cache): names, coordinates, size and type."
    )]
    async fn search_sky(
        &self,
        Parameters(args): Parameters<SkySearchArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = api::with_query(
            "/director/v1/sky/search",
            [
                ("q", Some(args.query)),
                ("limit", args.limit.map(|limit| limit.to_string())),
                ("online", args.online.then(|| "true".to_string())),
            ],
        );
        self.answer(&ctx, Method::GET, &path, None).await
    }

    // ---------- Planning: changes ----------

    #[tool(
        description = "Create a plan (a planning project) with a name. Needs write access. Returns its id and revision; save_framing and save_plan fill it in."
    )]
    async fn create_plan(
        &self,
        Parameters(args): Parameters<CreatePlanArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let body = json!({ "id": uuid::Uuid::new_v4(), "name": args.name });
        self.answer(&ctx, Method::POST, "/director/v1/projects", Some(body))
            .await
    }

    #[tool(description = "Rename a plan. Refuses if it was renamed since you read its revision.")]
    async fn rename_plan(
        &self,
        Parameters(args): Parameters<RenamePlanArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!("/director/v1/projects/{}", api::encode(&args.plan_id));
        let body = json!({ "name": args.name, "expected_revision": args.expected_revision });
        self.answer(&ctx, Method::PATCH, &path, Some(body)).await
    }

    #[tool(
        description = "Save a plan: send the whole object get_plan returned under `plan`, changed (goals, rigs, exposures per band). Refuses a stale revision; read it again and redo the change. Saving changes nothing on the rigs until the plan is activated."
    )]
    async fn save_plan(
        &self,
        Parameters(args): Parameters<SavePlanArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!("/director/v1/projects/{}/plan", api::encode(&args.plan_id));
        self.answer(&ctx, Method::PUT, &path, Some(args.plan)).await
    }

    #[tool(
        description = "Save a plan's framing: send the whole object get_framing returned under `draft`, changed (centre, rotation, mosaic, panel rigs). Refuses a stale revision."
    )]
    async fn save_framing(
        &self,
        Parameters(args): Parameters<SaveFramingArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/projects/{}/framing",
            api::encode(&args.plan_id)
        );
        self.answer(&ctx, Method::PUT, &path, Some(args.framing))
            .await
    }

    #[tool(
        description = "Take a plan's values from what a rig's Target Scheduler database already holds for it (exposures, templates, goals). Refuses if the plan or framing changed since the revisions you give."
    )]
    async fn take_plan_from_target_scheduler(
        &self,
        Parameters(args): Parameters<TakeFromSchedulerArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/projects/{}/plan/take-target-scheduler",
            api::encode(&args.plan_id)
        );
        let body = json!({
            "rig_id": args.rig,
            "plan_revision": args.plan_revision,
            "framing_revision": args.framing_revision,
        });
        self.answer(&ctx, Method::POST, &path, Some(body)).await
    }

    #[tool(
        description = "When a plan's target is observable from each rig's site over the coming nights: hours above the altitude limit, Moon, twilight. A calculation; it changes nothing, but needs write access as in the app."
    )]
    async fn check_feasibility(
        &self,
        Parameters(args): Parameters<FeasibilityArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/projects/{}/feasibility",
            api::encode(&args.plan_id)
        );
        self.answer(
            &ctx,
            Method::POST,
            &path,
            Some(json!({ "nights": args.nights })),
        )
        .await
    }

    #[tool(
        description = "Preview activating a plan: exactly what would be written to each rig's Target Scheduler database (projects, targets, exposure plans, templates, limits) and a `preview_digest`. Writes nothing. apply_activation takes the digest."
    )]
    async fn preview_activation(
        &self,
        Parameters(args): Parameters<ActivationArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/projects/{}/activation/preview",
            api::encode(&args.plan_id)
        );
        let body = json!({ "collaboration": args.collaboration });
        self.answer(&ctx, Method::POST, &path, Some(body)).await
    }

    #[tool(
        description = "Activate a plan: write what preview_activation showed into the rigs' Target Scheduler databases. Needs the preview's `preview_digest`; if anything changed since, it refuses and you preview again. Needs write access and database management, as in the app."
    )]
    async fn apply_activation(
        &self,
        Parameters(args): Parameters<ApplyActivationArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/projects/{}/activation/apply",
            api::encode(&args.plan_id)
        );
        let body = json!({
            "preview_digest": args.preview_digest,
            "collaboration": args.collaboration,
        });
        self.answer(&ctx, Method::POST, &path, Some(body)).await
    }

    #[tool(
        description = "Send an activated plan to rigs on other machines (remote rigs paired for sync), as the app's Push does. Reports each rig's outcome. Needs write access and database management."
    )]
    async fn push_activation(
        &self,
        Parameters(args): Parameters<PlanArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/projects/{}/activation/push",
            api::encode(&args.plan_id)
        );
        self.answer(&ctx, Method::POST, &path, Some(json!({})))
            .await
    }

    #[tool(
        description = "Write a rig's scheduling limits into every project of its Target Scheduler database, as get_rig_scheduling showed them. Needs that answer's `digest`; refuses if anything changed since. Needs write access and database management."
    )]
    async fn apply_rig_scheduling(
        &self,
        Parameters(args): Parameters<ApplySchedulingArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/rigs/{}/scheduling/apply",
            api::encode(&args.rig)
        );
        self.answer(
            &ctx,
            Method::POST,
            &path,
            Some(json!({ "digest": args.digest })),
        )
        .await
    }

    #[tool(
        description = "Add or change a template in the shared exposure template library. Refuses a stale revision."
    )]
    async fn save_template(
        &self,
        Parameters(args): Parameters<SaveTemplateArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!("/director/v1/templates/{}", api::encode(&args.template_id));
        self.answer(&ctx, Method::PUT, &path, Some(args.template))
            .await
    }

    #[tool(description = "Remove a template from the shared exposure template library.")]
    async fn delete_template(
        &self,
        Parameters(args): Parameters<TemplateIdArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = api::with_query(
            &format!("/director/v1/templates/{}", api::encode(&args.template_id)),
            [("revision", Some(args.revision.to_string()))],
        );
        self.answer(&ctx, Method::DELETE, &path, None).await
    }

    #[tool(
        description = "Save observing preferences for a scope (global, a site, or a rig): altitude, Moon, twilight and the like. Send the whole settings object, changed."
    )]
    async fn save_preferences(
        &self,
        Parameters(args): Parameters<SavePreferencesArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = format!(
            "/director/v1/preferences/{}/{}",
            api::encode(&args.scope),
            api::encode(&args.id)
        );
        self.answer(&ctx, Method::PUT, &path, Some(args.settings))
            .await
    }
}

#[tool_handler]
impl ServerHandler for PsfGuardMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("psf-guard", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "PSF Guard catalogs, grades and stacks astrophotography frames, and plans what \
                 rigs shoot next. Start with list_databases; catalog tools take a database id \
                 from it. Grades are accepted, rejected or pending. analyze_sequence and \
                 get_image_quality report evidence and suggestions; nothing changes until \
                 grade_images runs. Jobs (import, quality scan, WBPP) return at once; poll \
                 get_jobs for progress. For a stack, list_stacks names each channel's job; \
                 get_stack gives its frames and per-night drift, get_stack_calibration the \
                 masters it applied, and get_stack_image the picture (stretch `background` shows \
                 patterns). explain_calibration says why one light got, or missed, each master. \
                 Planning: list_plans and list_rigs give the ids get_plan, get_plan_progress, \
                 get_framing, get_activation and the rig tools take. To change a plan, read it, \
                 change the object and save it whole (save_plan, save_framing); activation is \
                 preview_activation, then apply_activation with its digest, then \
                 push_activation for remote rigs. api_routes lists every \
                 route api_get can read when no tool fits. Every call runs as you: your role \
                 and the server's database-management setting decide what it may change. \
                 Catalog predictions and header values are not pixel evidence; say which one \
                 a conclusion rests on."
                    .to_string(),
            )
    }
}

fn failure(message: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(message.to_string())])
}

fn text_result(value: &Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(api::text(value))])
}

/// When each of a stack's frames was taken, and where the catalog's nights
/// split. Read after the job itself was read as the caller.
fn capture_times(
    ctx: &DbContext,
    group: &StackGroupStatus,
) -> Result<(std::collections::HashMap<i32, i64>, i64), AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = crate::db::Database::new(&conn);
    let ids: Vec<i32> = group.frames.iter().map(|frame| frame.image_id).collect();
    let mut acquired = std::collections::HashMap::new();
    for chunk in ids.chunks(500) {
        for image in db.get_images_by_ids(chunk).map_err(AppError::db)? {
            if let Some(at) = image.acquired_date {
                acquired.insert(image.id, at);
            }
        }
    }
    let boundary = sky_coverage::catalog_night_boundary(&conn).unwrap_or(12 * 3600);
    Ok((acquired, boundary))
}

/// Drop the `*_url` fields, which only the app's own pages can follow.
fn strip_links(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|key, _| !key.ends_with("_url"));
            map.values_mut().for_each(strip_links);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_links),
        _ => {}
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

#[cfg(test)]
mod tests;
