//! Calibration tools: flat groups and their changes, what a group feeds,
//! the library's sessions and frames with their dust, a person's choices
//! (Use anyway, splits and joins, a light's side), and the calibration
//! jobs (flat measuring, frame sites, duplicates).
//!
//! Like every tool, each sends its request through the API router as the
//! caller, so the read-only role and the database-management gate apply
//! exactly as in the app. What the routes answer is cut down here to what
//! an agent reads: counts and the deepest few marks rather than every
//! spot's outline, and lists in pages.

use axum::http::Method;
use rmcp::{
    handler::server::wrapper::Parameters, model::CallToolResult, service::RequestContext, tool,
    tool_router, RoleServer,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{api, failure, text_result, PsfGuardMcp, ToolResult};

/// Rows a calibration list returns by default, and at most.
const DEFAULT_ROWS: usize = 50;
const MAX_ROWS: usize = 200;
/// The deepest dust marks a change keeps by name; the rest are counted.
const DEEPEST_SPOTS: usize = 3;
/// Light ids a project's row in `get_flat_group_use` keeps.
const GROUP_USE_IDS: usize = 200;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FlatGroupsArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Only the families this project's lights touch.
    pub project_id: Option<i32>,
    /// Only this rig: its `rig_uuid`, or part of its name.
    pub rig: Option<String>,
    /// Return the route's whole answer: the outlines of each change's
    /// deepest 12 dust marks and every pair of a group's sessions included.
    /// Large; leave it off unless the summary is not enough.
    #[serde(default)]
    pub full: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FlatGroupUseArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// One group, by its `key` from `get_flat_groups` or one of its
    /// sessions' `response_key`. Omitted gives every group.
    pub group: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CalibrationListArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Only what this project's lights use.
    pub project_id: Option<i32>,
    /// Only this rig, by `rig_uuid`.
    pub rig: Option<String>,
    /// `bias`, `dark`, `dark_flat` or `flat`.
    pub kind: Option<String>,
    /// Only this filter's flats, such as `Ha`.
    pub filter: Option<String>,
    /// Frames only: one session, by its `id` from `list_calibration_sessions`.
    pub session: Option<String>,
    /// Frames only: one flat group's flats, by its `key` from
    /// `get_flat_groups` or one of its sessions' `response_key`.
    pub group: Option<String>,
    /// `ok`, `suspect`, `poor` or `ungraded`.
    pub grade: Option<String>,
    /// Only frames kept out of masters (true) or only those used (false).
    pub kept_out: Option<bool>,
    /// Nights from and to, `YYYY-MM-DD`.
    pub night_from: Option<String>,
    pub night_to: Option<String>,
    /// Page number from 0.
    pub page: Option<usize>,
    /// Rows a page, at most 200 (default 50).
    pub per_page: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CalibrationFrameArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// The frame's `frame_uuid` from `list_calibration_frames`.
    pub frame_uuid: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DuplicatesArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Exposures of the plan to list (default 20, at most 200); the
    /// counts cover the rest.
    pub limit: Option<usize>,
    /// Exposures of the plan to skip, for paging.
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RepairDuplicatesArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// The plan's `token` from `get_calibration_duplicates`, after a check.
    pub token: String,
}

/// What Use anyway, Keep out or Clear does to a calibration frame.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FrameChoiceArg {
    /// Use the frame whatever its grade: a suspect or kept-out frame
    /// counts as OK.
    Use,
    /// Keep it out of masters and exports.
    Exclude,
    /// Clear the choice; the grade decides again.
    Clear,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FrameChoiceArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Calibration frame UUIDs, from `list_calibration_frames`.
    pub frame_uuids: Vec<String>,
    pub choice: FrameChoiceArg,
}

/// A person's split or join of a flat family's groups.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GroupChoiceArg {
    /// Start a new group at `after_response`.
    Split,
    /// Join `after_response`'s group with `before_response`'s.
    Join,
    /// Undo a split or join, named by `anchor_frame` or `after_response`.
    Reset,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GroupOverrideArgs {
    /// Database id (slug) or name.
    pub database: String,
    pub choice: GroupChoiceArg,
    /// The session the edge comes before, by `response_key` from
    /// `get_flat_groups`: the first session of the later group.
    pub after_response: Option<String>,
    /// For a join: the session on the earlier side, by `response_key`, of
    /// the same family.
    pub before_response: Option<String>,
    /// For a reset: the choice's `anchor_frame`, as `get_flat_groups` shows
    /// it under `choice` or `orphaned`.
    pub anchor_frame: Option<String>,
}

/// Which flats some lights take around a measured change.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LightChoiceArg {
    /// Flats from before the change `change_key`.
    Before,
    /// Flats from after it.
    After,
    /// One group's flats, named by `group_response`.
    Group,
    /// Clear the lights' choices: for `change_key` only, or all of them.
    Reset,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LightChoiceArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Light ids from `list_images`.
    pub image_ids: Vec<i64>,
    /// Every light of the imaging sessions those lights belong to.
    #[serde(default)]
    pub whole_session: bool,
    pub choice: LightChoiceArg,
    /// The change a side is for, by its `key` from `get_light_flat_options`
    /// or `get_flat_groups` events; for a reset, only that change's choice.
    pub change_key: Option<String>,
    /// For `group`: one of the group's sessions, by `response_key`.
    pub group_response: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LightIdsArgs {
    /// Database id (slug) or name.
    pub database: String,
    /// Light ids from `list_images`.
    pub image_ids: Vec<i64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CalibrationJobArgs {
    /// Database id (slug) or name.
    pub database: String,
}

impl PsfGuardMcp {
    /// `/db/{db}` and the rest, for a database the caller named.
    fn db_path(&self, database: &str, rest: &str) -> Result<String, CallToolResult> {
        let db = self.database(database)?;
        Ok(format!("/db/{}{rest}", api::encode(&db.id)))
    }
}

#[tool_router(router = calibration_tools, vis = "pub(super)")]
impl PsfGuardMcp {
    // ---------- Flat groups ----------

    #[tool(
        description = "Each rig's flat groups, from what flat measuring found: per rig its limits, angle term and clock (its UTC offset over time, from its night_zone setting or its frames' headers); per family (filter and measured angle range) its groups newest first, each with its nights, flats and sessions by `response_key`, and the splits between groups with why (dust, light pattern or vignetting against the limits), the dust they show (every mark of the comparison counted by kind and shape, dust that moved only within focus travel apart, and the deepest three named), and any split or join a person made (`choice`, with whether the measurements agree, or why it no longer applies). Also the rig's change events, its unmeasured flat sessions, and orphaned choices. Reads no pixels."
    )]
    async fn get_flat_groups(
        &self,
        Parameters(args): Parameters<FlatGroupsArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let base = attempt!(self.db_path(&args.database, "/calibrations/flat-groups"));
        let path = api::with_query(
            &base,
            [("project", args.project_id.map(|id| id.to_string()))],
        );
        let mut view = attempt!(self.get(&ctx, &path).await);
        if let Some(wanted) = args.rig.as_deref().map(str::trim).filter(|r| !r.is_empty())
            && let Some(rigs) = view.get_mut("rigs").and_then(Value::as_array_mut)
        {
            rigs.retain(|rig| rig_matches(rig, wanted));
        }
        Ok(text_result(&if args.full {
            view
        } else {
            summarize_flat_groups(&view)
        }))
    }

    #[tool(
        description = "What flat groups feed: per group, the flat masters built from it and the lights whose flat master it is, counted by how their flats were chosen (own side, another angle, corrected across a change, a person's choice, undetermined) and by project, with the light ids. Worked out in the background: `computed` false or `refreshing` true means ask again shortly."
    )]
    async fn get_flat_group_use(
        &self,
        Parameters(args): Parameters<FlatGroupUseArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let base = attempt!(self.db_path(&args.database, "/calibrations/flat-groups/use"));
        let path = api::with_query(&base, [("group", args.group)]);
        let mut view = attempt!(self.get(&ctx, &path).await);
        for group in view
            .get_mut("groups")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            for project in group
                .get_mut("projects")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                cap_list(project, "image_ids", GROUP_USE_IDS);
                cap_list(project, "flagged_ids", GROUP_USE_IDS);
            }
        }
        Ok(text_result(&view))
    }

    #[tool(
        description = "What one project's lights use from the calibration library: how many lights were matched, the sessions and recorded masters they use, the rigs, and how many headers are still being read (`pending`; ask again while it is above 0). Session ids work with list_calibration_frames."
    )]
    async fn get_calibration_project_scope(
        &self,
        Parameters(args): Parameters<super::ProjectArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let base = attempt!(self.db_path(&args.database, "/calibrations/project-scope"));
        let path = api::with_query(&base, [("project", Some(args.project_id.to_string()))]);
        self.answer(&ctx, Method::GET, &path, None).await
    }

    #[tool(
        description = "Calibration sessions (frames of one kind from one night and one set of settings), newest night first, a page at a time: kind, night, rig, filter, exposure, temperatures, gain, rotation, and frames by grade. Narrow by project, rig, kind, filter, grade or nights. Also the values the filters can take."
    )]
    async fn list_calibration_sessions(
        &self,
        Parameters(args): Parameters<CalibrationListArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let base = attempt!(self.db_path(&args.database, "/calibrations/sessions"));
        let path = api::with_query(&base, list_query(&args));
        let mut page = attempt!(self.get(&ctx, &path).await);
        prune_nulls(&mut page);
        Ok(text_result(&page))
    }

    #[tool(
        description = "Calibration frames a page at a time: file, night, session, kind, settings, sensor temperature, rotation, grade and its finding, any person's choice (Use anyway or kept out), and whether the file is on disk. Narrow by project, rig, kind, filter, session, flat group, grade, kept out or nights; a flat group's flats come night by night."
    )]
    async fn list_calibration_frames(
        &self,
        Parameters(args): Parameters<CalibrationListArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let base = attempt!(self.db_path(&args.database, "/calibrations/frames"));
        let path = api::with_query(&base, list_query(&args));
        let mut page = attempt!(self.get(&ctx, &path).await);
        prune_nulls(&mut page);
        Ok(text_result(&page))
    }

    #[tool(
        description = "One calibration frame: its grade, findings and measurements beside its session's, and for a flat its dust: the spots and large dust (disks, donuts, patches) it holds, with depth and, from the header's focal ratio, distance in front of the sensor; and its session's dust, found on the stack of the session's flats, with which of those marks this flat shows and what it holds alone (`transients`, not dust)."
    )]
    async fn get_calibration_frame(
        &self,
        Parameters(args): Parameters<CalibrationFrameArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = attempt!(self.db_path(
            &args.database,
            &format!("/calibrations/frames/{}", api::encode(&args.frame_uuid)),
        ));
        let mut frame = attempt!(self.get(&ctx, &path).await);
        compact_frame(&mut frame);
        Ok(text_result(&frame))
    }

    #[tool(
        description = "The changes some lights were shot during, which a side can be chosen for: each change's key, window, what changed, its filters, how many of the lights fall in it and how many a person already put before or after it. Reads no header or pixel. Needs write access, as the app's Images action does."
    )]
    async fn get_light_flat_options(
        &self,
        Parameters(args): Parameters<LightIdsArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = attempt!(self.db_path(&args.database, "/calibrations/flat-choices/options"));
        let body = json!({ "image_ids": args.image_ids });
        self.answer(&ctx, Method::POST, &path, Some(body)).await
    }

    // ---------- A person's choices ----------

    #[tool(
        description = "Use anyway, keep out, or clear the choice on calibration frames. `use` makes a suspect or kept-out frame count as OK for masters and health; `exclude` keeps it out of masters and exports; `clear` lets its grade decide again. Masters already built stay as they are. Needs write access."
    )]
    async fn set_calibration_frame_choice(
        &self,
        Parameters(args): Parameters<FrameChoiceArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = attempt!(self.db_path(&args.database, "/calibrations/frames/quality"));
        let body = json!({ "frame_uuids": args.frame_uuids, "choice": args.choice });
        self.answer(&ctx, Method::PUT, &path, Some(body)).await
    }

    #[tool(
        description = "Split a flat family's groups at a session, join two neighbouring groups, or reset such a choice. A split starts a new group at `after_response`; a join merges `after_response`'s group with `before_response`'s; a reset names the choice by `anchor_frame` or `after_response`. Lights are asked again where a change moved; no flat is measured again. Needs write access and database management, as in the app."
    )]
    async fn set_flat_group_override(
        &self,
        Parameters(args): Parameters<GroupOverrideArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = attempt!(self.db_path(&args.database, "/calibrations/flat-groups/overrides"));
        let body = json!({
            "choice": args.choice,
            "after_response": args.after_response,
            "before_response": args.before_response,
            "anchor_frame": args.anchor_frame,
        });
        self.answer(&ctx, Method::PUT, &path, Some(body)).await
    }

    #[tool(
        description = "Choose which flats lights take around a measured change: `before` or `after` the change `change_key`, one group's flats (`group`, by `group_response`), or `reset` their choices. Lights not shot during the change, or of another rig or filter, are skipped and counted. Needs write access and database management, as in the app."
    )]
    async fn set_light_flat_choice(
        &self,
        Parameters(args): Parameters<LightChoiceArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = attempt!(self.db_path(&args.database, "/calibrations/flat-choices"));
        let body = json!({
            "image_ids": args.image_ids,
            "whole_session": args.whole_session,
            "choice": args.choice,
            "change_key": args.change_key,
            "group_response": args.group_response,
        });
        self.answer(&ctx, Method::PUT, &path, Some(body)).await
    }

    // ---------- Calibration jobs ----------

    #[tool(
        description = "Where the calibration duplicate check or repair stands, and the latest plan: exposures the library records more than once, what a repair would merge, the masters it would remove, and the plan's `token`. `stale` means the library changed since the check; check again. Lists the plan's exposures a page at a time."
    )]
    async fn get_calibration_duplicates(
        &self,
        Parameters(args): Parameters<DuplicatesArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = attempt!(self.db_path(&args.database, "/calibrations/duplicates"));
        let mut status = attempt!(self.get(&ctx, &path).await);
        let limit = args.limit.unwrap_or(20).min(MAX_ROWS);
        compact_duplicates(&mut status, args.offset.unwrap_or(0), limit);
        Ok(text_result(&status))
    }

    #[tool(
        description = "Check the calibration library for exposures recorded more than once. Reads rows and files and changes nothing, so a read-only account may run it. Returns at once; poll get_calibration_duplicates for the plan and its token."
    )]
    async fn check_calibration_duplicates(
        &self,
        Parameters(args): Parameters<CalibrationJobArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = attempt!(self.db_path(&args.database, "/calibrations/duplicates/check"));
        let mut status = attempt!(self.send(&ctx, Method::POST, &path, None).await);
        compact_duplicates(&mut status, 0, 0);
        Ok(text_result(&status))
    }

    #[tool(
        description = "Repair calibration duplicates: merge the rows of the plan a check made, in one transaction, and remove the masters built from them so they are built again. Files are never moved or deleted. Needs the plan's `token` from get_calibration_duplicates: if the library changed since the check, it applies nothing and says what changed. Returns at once; poll get_calibration_duplicates. Needs write access and database management."
    )]
    async fn repair_calibration_duplicates(
        &self,
        Parameters(args): Parameters<RepairDuplicatesArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        if args.token.trim().is_empty() {
            return Ok(failure(
                "A repair needs the plan's token: run check_calibration_duplicates, then read \
                 it from get_calibration_duplicates",
            ));
        }
        let path = attempt!(self.db_path(&args.database, "/calibrations/duplicates/repair"));
        let body = json!({ "token": args.token });
        let mut status = attempt!(self.send(&ctx, Method::POST, &path, Some(body)).await);
        compact_duplicates(&mut status, 0, 0);
        Ok(text_result(&status))
    }

    #[tool(
        description = "Stop the calibration duplicate check or repair running. A repair stopped part way rolls back whole. Needs write access."
    )]
    async fn cancel_calibration_duplicates(
        &self,
        Parameters(args): Parameters<CalibrationJobArgs>,
        ctx: RequestContext<RoleServer>,
    ) -> ToolResult {
        let path = attempt!(self.db_path(&args.database, "/calibrations/duplicates/stop"));
        let mut status = attempt!(self.send(&ctx, Method::POST, &path, None).await);
        compact_duplicates(&mut status, 0, 0);
        Ok(text_result(&status))
    }
}

/// The query of a calibration list, with its page size held in bounds.
fn list_query(args: &CalibrationListArgs) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("project", args.project_id.map(|id| id.to_string())),
        ("rig", args.rig.clone()),
        ("kind", args.kind.clone()),
        ("filter", args.filter.clone()),
        ("session", args.session.clone()),
        ("group", args.group.clone()),
        ("grade", args.grade.clone()),
        ("kept_out", args.kept_out.map(|kept| kept.to_string())),
        ("night_from", args.night_from.clone()),
        ("night_to", args.night_to.clone()),
        ("page", args.page.map(|page| page.to_string())),
        (
            "per_page",
            Some(
                args.per_page
                    .unwrap_or(DEFAULT_ROWS)
                    .clamp(1, MAX_ROWS)
                    .to_string(),
            ),
        ),
    ]
}

/// Whether a flat-groups rig is the one asked for: by its uuid, or by part
/// of its name.
fn rig_matches(rig: &Value, wanted: &str) -> bool {
    rig["rig_uuid"].as_str() == Some(wanted)
        || rig["name"]
            .as_str()
            .is_some_and(|name| name.to_lowercase().contains(&wanted.to_lowercase()))
}

/// The named fields of an object that are there, in order.
fn pick(value: &Value, keys: &[&str]) -> Value {
    let mut out = Map::new();
    for key in keys {
        if let Some(field) = value.get(*key)
            && !field.is_null()
        {
            out.insert((*key).to_string(), field.clone());
        }
    }
    Value::Object(out)
}

fn items(value: &Value, key: &str) -> Vec<Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// A change's dust, summed up: marks counted and those within focus
/// travel, by kind (`new`, `gone`, `moved`) and shape, and the deepest few
/// that count. The route lists only the deepest
/// [`crate::server::flat_groups::LISTED_SPOTS`] spots and counts every one
/// in `totals`; the counts come from those. Without them the counts are of
/// the list alone, and a full list says it may have been cut.
pub(super) fn summarize_spots(spots: &[Value], totals: Option<&Value>) -> Value {
    let mut summary = count_listed_spots(spots);
    match totals.filter(|totals| totals.is_object()) {
        Some(totals) => {
            for (from, to) in [
                ("counted", "counted"),
                ("within_tolerance", "within_focus_travel"),
                ("by_kind", "by_kind"),
                ("by_shape", "by_shape"),
            ] {
                if let Some(value) = totals.get(from) {
                    summary[to] = value.clone();
                }
            }
            summary["listed"] = json!(spots.len());
        }
        None if spots.len() >= crate::server::flat_groups::LISTED_SPOTS => {
            summary["listed_at_most"] = json!(crate::server::flat_groups::LISTED_SPOTS);
        }
        None => {}
    }
    summary
}

/// [`summarize_spots`] of the listed spots alone.
fn count_listed_spots(spots: &[Value]) -> Value {
    let mut by_kind = std::collections::BTreeMap::<String, usize>::new();
    let mut by_shape = std::collections::BTreeMap::<String, usize>::new();
    let mut counted: Vec<&Value> = Vec::new();
    let mut within = 0;
    for spot in spots {
        if spot["within_tolerance"].as_bool() == Some(true) {
            within += 1;
            continue;
        }
        counted.push(spot);
        let text = |key: &str| spot[key].as_str().unwrap_or("unknown").to_string();
        *by_kind.entry(text("kind")).or_default() += 1;
        *by_shape.entry(text("shape")).or_default() += 1;
    }
    counted.sort_by(|a, b| {
        let depth = |spot: &Value| spot["peak_pct"].as_f64().unwrap_or(0.0).abs();
        depth(b).total_cmp(&depth(a))
    });
    let deepest: Vec<Value> = counted
        .iter()
        .take(DEEPEST_SPOTS)
        .map(|spot| {
            pick(
                spot,
                &[
                    "kind",
                    "shape",
                    "darker",
                    "x",
                    "y",
                    "radius",
                    "peak_pct",
                    "offset_px",
                ],
            )
        })
        .collect();
    json!({
        "counted": counted.len(),
        "within_focus_travel": within,
        "by_kind": by_kind,
        "by_shape": by_shape,
        "deepest": deepest,
    })
}

fn compact_vignetting(change: &Value) -> Vec<Value> {
    items(change, "vignetting")
        .iter()
        .map(|v| pick(v, &["edge", "darker", "depth_pct"]))
        .collect()
}

fn compact_choice(choice: &Value) -> Value {
    let mut out = pick(
        choice,
        &[
            "choice",
            "label",
            "anchor_frame",
            "chosen_at",
            "agrees",
            "orphaned",
        ],
    );
    if let Some(conflict) = choice.get("conflict").filter(|c| !c.is_null()) {
        out["conflict"] = pick(conflict, &["key", "what", "filters"]);
    }
    out
}

fn compact_split(split: &Value) -> Value {
    let mut out = pick(
        split,
        &[
            "before_response",
            "after_response",
            "before_night",
            "after_night",
            "dust_pct",
            "broad_pct",
            "vignetting_pct",
            "reason",
            "event_key",
            "by_hand",
        ],
    );
    out["dust"] = summarize_spots(&items(split, "spots"), split.get("spot_totals"));
    let vignetting = compact_vignetting(split);
    if !vignetting.is_empty() {
        out["vignetting"] = json!(vignetting);
    }
    if let Some(choice) = split.get("choice").filter(|c| !c.is_null()) {
        out["choice"] = compact_choice(choice);
    }
    out
}

fn compact_group(group: &Value) -> Value {
    let mut out = pick(
        group,
        &["key", "first_night", "last_night", "frames", "widest"],
    );
    out["sessions"] = json!(items(group, "sessions")
        .iter()
        .map(|session| pick(session, &["response_key", "night", "frames", "noise_pct"]))
        .collect::<Vec<_>>());
    out
}

fn compact_event(event: &Value) -> Value {
    let mut out = pick(
        event,
        &[
            "key",
            "start",
            "end",
            "what",
            "dust_pct",
            "broad_pct",
            "vignetting_pct",
            "camera_side",
            "filters",
            "rotations",
            "before_response",
            "after_response",
            "within_flat_run",
        ],
    );
    out["dust"] = summarize_spots(&items(event, "spots"), event.get("spot_totals"));
    let vignetting = compact_vignetting(event);
    if !vignetting.is_empty() {
        out["vignetting"] = json!(vignetting);
    }
    out
}

/// The flat-groups view an agent reads: every group, split, join and
/// change, with the dust each shows summed up rather than outlined.
pub(super) fn summarize_flat_groups(view: &Value) -> Value {
    let rigs: Vec<Value> = items(view, "rigs")
        .iter()
        .map(|rig| {
            let mut out = pick(
                rig,
                &[
                    "rig_uuid",
                    "name",
                    "measured",
                    "dust_limit_pct",
                    "pattern_limit_pct",
                    "own_limits",
                    "angle_term",
                    "angle_pairs",
                    "clock",
                ],
            );
            out["families"] = json!(items(rig, "families")
                .iter()
                .map(|family| {
                    let mut out = pick(family, &["key", "filter", "rotation", "angle_label"]);
                    out["groups"] = json!(items(family, "groups")
                        .iter()
                        .map(compact_group)
                        .collect::<Vec<_>>());
                    out["splits"] = json!(items(family, "splits")
                        .iter()
                        .map(compact_split)
                        .collect::<Vec<_>>());
                    let joins: Vec<Value> =
                        items(family, "joins").iter().map(compact_split).collect();
                    if !joins.is_empty() {
                        out["joins"] = json!(joins);
                    }
                    out
                })
                .collect::<Vec<_>>());
            out["events"] = json!(items(rig, "events")
                .iter()
                .map(compact_event)
                .collect::<Vec<_>>());
            out["unmeasured"] = json!(items(rig, "unmeasured")
                .iter()
                .map(|session| pick(
                    session,
                    &["night", "filter", "rotation", "frames", "reason"]
                ))
                .collect::<Vec<_>>());
            let orphaned: Vec<Value> = items(rig, "orphaned").iter().map(compact_choice).collect();
            if !orphaned.is_empty() {
                out["orphaned"] = json!(orphaned);
            }
            out
        })
        .collect();
    json!({ "available": view["available"], "rigs": rigs })
}

/// Keep the first `max` entries of a list field, and say how many there
/// were when it was cut.
pub(super) fn cap_list(value: &mut Value, key: &str, max: usize) {
    let Some(list) = value.get_mut(key).and_then(Value::as_array_mut) else {
        return;
    };
    let total = list.len();
    if total > max {
        list.truncate(max);
        value[format!("{key}_total")] = json!(total);
    }
}

/// Drop null fields, which say nothing to an agent, everywhere.
pub(super) fn prune_nulls(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.retain(|_, field| !field.is_null());
            map.values_mut().for_each(prune_nulls);
        }
        Value::Array(list) => list.iter_mut().for_each(prune_nulls),
        _ => {}
    }
}

/// One dust feature without its outline's corners and pixel count.
fn compact_feature(feature: &mut Value) {
    if let Some(map) = feature.as_object_mut() {
        for key in ["left", "top", "right", "bottom", "pixels", "bounds"] {
            map.remove(key);
        }
    }
}

/// A calibration frame's detail with its dust marks kept but their
/// outlines dropped, and the app's links removed.
pub(super) fn compact_frame(frame: &mut Value) {
    super::strip_links(frame);
    prune_nulls(frame);
    for holder in ["measurement", "session_dust"] {
        let Some(holder) = frame.get_mut(holder) else {
            continue;
        };
        for key in ["features", "transients", "spots"] {
            for feature in holder
                .get_mut(key)
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                compact_feature(feature);
            }
        }
    }
}

/// A duplicate status with the plan's exposures paged and summed up, and
/// its masters counted.
pub(super) fn compact_duplicates(status: &mut Value, offset: usize, limit: usize) {
    let Some(report) = status.get_mut("report").filter(|r| r.is_object()) else {
        return;
    };
    let groups = items(report, "groups");
    report["groups_total"] = json!(groups.len());
    report["groups"] = json!(groups
        .iter()
        .skip(offset)
        .take(limit)
        .map(|group| {
            let mut out = pick(
                group,
                &[
                    "kind",
                    "file_name",
                    "rig_name",
                    "captured_at",
                    "keep",
                    "path",
                    "repointed",
                    "physical",
                ],
            );
            out["copies"] = json!(items(group, "members").len());
            out
        })
        .collect::<Vec<_>>());
    if limit > 0 {
        report["offset"] = json!(offset);
    }
    report["masters_total"] = json!(items(report, "masters").len());
    if let Some(masters) = report.get_mut("masters").and_then(Value::as_array_mut) {
        masters.truncate(20);
    }
}
