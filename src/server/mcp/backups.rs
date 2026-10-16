//! Backup tools: the schedule, the archives and the run under way; back up
//! now, and stop.
//!
//! The routes sit behind the database-management gate and need an editor,
//! and an API token may never ask for secrets (#883). An agent is held at
//! least as strictly whatever it signs in with: these tools never ask for
//! secrets, never name a server folder of the agent's own choosing, and
//! never hand an archive out. `api_get` refuses the download route too.

use axum::http::Method;
use rmcp::{
    handler::server::wrapper::Parameters, service::RequestContext, tool, tool_router, RoleServer,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{failure, text_result, PsfGuardMcp, ToolResult};

/// Archives `get_backups` lists by default, and at most.
const DEFAULT_ARCHIVES: usize = 20;
const MAX_ARCHIVES: usize = 200;

/// What a tool says when asked for secrets.
pub(super) const NO_SECRETS: &str = "Backups made through MCP never include secrets (peer keys, \
     collaboration credentials, server tokens). A person signed in to the server, or in the \
     desktop app, makes one with secrets from Settings › Backups.";

/// What a backup copies.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupScopeArg {
    /// PSF Guard's settings, accounts and stores: small.
    Meta,
    /// The settings and every registered catalog.
    #[default]
    Full,
}

/// Where a backup goes.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BackupDestinationArg {
    /// The server's download area: a person downloads it from Settings ›
    /// Backups.
    #[default]
    Downloads,
    /// The folder the backup schedule writes to.
    ScheduleFolder,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BackupsArgs {
    /// Archives to list, newest first (default 20, at most 200).
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StartBackupArgs {
    /// `full` (default) or `meta`.
    #[serde(default)]
    pub scope: BackupScopeArg,
    /// `downloads` (default) or `schedule_folder`.
    #[serde(default)]
    pub destination: BackupDestinationArg,
    /// Always refused: an agent never makes a backup with secrets.
    #[serde(default)]
    pub include_secrets: bool,
}

#[tool_router(router = backup_tools, vis = "pub(super)")]
impl PsfGuardMcp {
    #[tool(
        description = "Backups: the schedule (on, how often, folder, how many kept), when the next run is due, the run under way and how the last one went, the archives newest first (scope, size, catalogs, made by hand or on schedule, whether it holds secrets), folder warnings, and what a full or meta backup would copy now. Needs write access and database management, as Settings › Backups does."
    )]
    async fn get_backups(
        &self,
        Parameters(args): Parameters<BackupsArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let mut overview = attempt!(self.get(&ctx, "/backups").await);
        let limit = args
            .limit
            .unwrap_or(DEFAULT_ARCHIVES)
            .clamp(1, MAX_ARCHIVES);
        compact_backups(&mut overview, limit);
        Ok(text_result(&overview))
    }

    #[tool(
        description = "Back up now: the server's settings and stores (`meta`) or those and every catalog (`full`), to the download area or the schedule's folder. Never with secrets. One backup runs at a time; it returns at once, and get_backups or get_activity follows it. Needs write access and database management."
    )]
    async fn start_backup(
        &self,
        Parameters(args): Parameters<StartBackupArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        if args.include_secrets {
            return Ok(failure(NO_SECRETS));
        }
        let destination =
            match args.destination {
                BackupDestinationArg::Downloads => None,
                BackupDestinationArg::ScheduleFolder => {
                    let overview = attempt!(self.get(&ctx, "/backups").await);
                    match overview["settings"]["destination"]
                        .as_str()
                        .map(str::trim)
                        .filter(|folder| !folder.is_empty())
                    {
                        Some(folder) => Some(folder.to_string()),
                        None => return Ok(failure(
                            "The backup schedule has no folder; a person sets one in Settings › \
                             Backups. Back up to `downloads` instead.",
                        )),
                    }
                }
            };
        let body = json!({
            "scope": args.scope,
            "include_secrets": false,
            "destination": destination,
        });
        self.answer(&ctx, Method::POST, "/backups/run", Some(body))
            .await
    }

    #[tool(
        description = "Stop the backup running: it ends at its next step and leaves nothing of itself in the folder. Needs write access and database management."
    )]
    async fn stop_backup(&self, ctx: RequestContext<RoleServer>) -> ToolResult {
        self.answer(&ctx, Method::POST, "/backups/stop", None).await
    }
}

/// The backups overview with the archives listed newest first up to
/// `limit`, and no download paths: an agent cannot fetch an archive.
pub(super) fn compact_backups(overview: &mut Value, limit: usize) {
    let Some(archives) = overview.get_mut("backups").and_then(Value::as_array_mut) else {
        return;
    };
    let total = archives.len();
    archives.truncate(limit);
    for archive in archives.iter_mut() {
        if let Some(map) = archive.as_object_mut() {
            map.remove("download_path");
        }
    }
    overview["backups_total"] = json!(total);
}
