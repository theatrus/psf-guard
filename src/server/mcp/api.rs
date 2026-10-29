//! How the MCP tools reach the API: an in-process request through the same
//! `/api` router the UI uses, carrying the caller's own credentials. Every
//! gate a person meets (sign-in, the read-only role, database management, a
//! handler's own checks) applies to an agent the same way, by construction.

use axum::{
    body::Body,
    http::{header, request::Parts, HeaderMap, Method, Request, StatusCode},
};
use rmcp::{service::RequestContext, RoleServer};
use serde_json::Value;
use tower::ServiceExt;

use crate::server::state::AppState;

/// The largest answer a tool passes on as text.
const MAX_TEXT_BYTES: usize = 1024 * 1024;
/// The largest body read back from the router (stack previews included).
const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;

/// The headers that say who is calling, copied from the MCP request.
#[derive(Debug, Clone, Default)]
pub(super) struct Caller {
    headers: HeaderMap,
}

impl Caller {
    pub(super) fn of(ctx: &RequestContext<RoleServer>) -> Self {
        let mut headers = HeaderMap::new();
        if let Some(parts) = ctx.extensions.get::<Parts>() {
            for name in [header::AUTHORIZATION, header::COOKIE, header::HOST] {
                if let Some(value) = parts.headers.get(&name) {
                    headers.insert(name, value.clone());
                }
            }
        }
        Self { headers }
    }
}

/// What the router answered.
pub(super) enum Reply {
    Json(StatusCode, Value),
    Bytes {
        status: StatusCode,
        content_type: String,
        body: axum::body::Bytes,
    },
}

impl Reply {
    pub(super) fn status(&self) -> StatusCode {
        match self {
            Self::Json(status, _) | Self::Bytes { status, .. } => *status,
        }
    }
}

/// Send one request through the API router as the caller. `path` is below
/// `/api`, such as `/db/rig/images`, and may carry a query.
pub(super) async fn send(
    state: &AppState,
    caller: &Caller,
    method: Method,
    path: &str,
    body: Option<&Value>,
) -> Result<Reply, String> {
    let router = state
        .api_router()
        .ok_or_else(|| "The API is still starting; try again shortly".to_string())?;
    let mut request = Request::builder().method(method).uri(path);
    for (name, value) in &caller.headers {
        request = request.header(name, value);
    }
    let body = match body {
        Some(value) => {
            request = request.header(header::CONTENT_TYPE, "application/json");
            Body::from(value.to_string())
        }
        None => Body::empty(),
    };
    let request = request
        .body(body)
        .map_err(|error| format!("Could not build the request for {path}: {error}"))?;
    let response = router
        .oneshot(request)
        .await
        .map_err(|error| format!("The request to {path} failed: {error}"))?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = axum::body::to_bytes(response.into_body(), MAX_BODY_BYTES)
        .await
        .map_err(|error| format!("Could not read the answer from {path}: {error}"))?;
    if (content_type.starts_with("application/json") || bytes.first() == Some(&b'{'))
        && let Ok(value) = serde_json::from_slice(&bytes)
    {
        return Ok(Reply::Json(status, value));
    }
    Ok(Reply::Bytes {
        status,
        content_type,
        body: bytes,
    })
}

/// The data of a JSON answer, or why there is none. The API wraps answers as
/// `{success, data, error}`; a route that answers plain JSON passes as is.
pub(super) fn data(reply: Reply) -> Result<Value, String> {
    match reply {
        Reply::Json(status, value) => {
            let wrapped = value.get("success").is_some();
            let failed = !status.is_success()
                || (wrapped && value.get("success") == Some(&Value::Bool(false)));
            if failed {
                let message = value
                    .get("error")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| describe_status(status));
                return Err(message);
            }
            if wrapped {
                match value.get("data") {
                    Some(Value::Null) | None => Err(
                        "The catalog cache is still loading for this database; try again shortly"
                            .to_string(),
                    ),
                    Some(data) => Ok(data.clone()),
                }
            } else {
                Ok(value)
            }
        }
        Reply::Bytes {
            status,
            content_type,
            body,
        } => {
            if !status.is_success() {
                let text = String::from_utf8_lossy(&body);
                return Err(if text.trim().is_empty() {
                    describe_status(status)
                } else {
                    text.trim().to_string()
                });
            }
            Err(format!(
                "This route answers {} ({} bytes), not JSON",
                if content_type.is_empty() {
                    "binary data"
                } else {
                    content_type.as_str()
                },
                body.len()
            ))
        }
    }
}

fn describe_status(status: StatusCode) -> String {
    match status {
        StatusCode::NOT_FOUND => "Not found".to_string(),
        StatusCode::UNAUTHORIZED => "Sign in or send an API token to use this server".to_string(),
        StatusCode::FORBIDDEN => "This account may not do that".to_string(),
        status => format!(
            "The server answered {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or_default()
        ),
    }
}

/// JSON as tool text, cut at [`MAX_TEXT_BYTES`] with a note to narrow the
/// request.
pub(super) fn text(value: &Value) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut cut = MAX_TEXT_BYTES;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!(
        "{}\n… [cut at {MAX_TEXT_BYTES} of {} bytes; narrow the request with a limit, filter or offset]",
        &text[..cut],
        text.len()
    )
}

/// Percent-encode one query value or path segment.
pub(super) fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `path?key=value&…` for the parameters that are set.
pub(super) fn with_query<'a>(
    path: &str,
    params: impl IntoIterator<Item = (&'a str, Option<String>)>,
) -> String {
    let query: Vec<String> = params
        .into_iter()
        .filter_map(|(key, value)| value.map(|value| format!("{key}={}", encode(&value))))
        .collect();
    if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", query.join("&"))
    }
}

/// Whether `api_get` may read this path: below `/api`, not the MCP
/// endpoint itself, and nothing that climbs out of it.
pub(super) fn readable_path(path: &str) -> Result<(), String> {
    if !path.starts_with('/') {
        return Err("The path starts with `/`, below /api, such as `/db/<id>/images`".into());
    }
    let route = path.split('?').next().unwrap_or(path);
    if route == "/mcp" || route.starts_with("/mcp/") {
        return Err("The MCP endpoint is not a route to read".into());
    }
    if route
        .split('/')
        .any(|segment| segment == ".." || segment == ".")
    {
        return Err("The path may not contain `.` or `..` segments".into());
    }
    // An archive holds every catalog and, perhaps, the server's secrets: it
    // is a person's to download, never an agent's. The backup tools read
    // what an agent may see; `api_get` reads no backup route, under any
    // spelling of it.
    let spelled = normalized_route(route);
    if spelled
        .split('/')
        .any(|segment| segment == ".." || segment == ".")
    {
        return Err("The path may not contain `.` or `..` segments".into());
    }
    if spelled == "/backups" || spelled.starts_with("/backups/") {
        return Err(
            "Backups are read with get_backups and get_activity; archives are not handed to agents"
                .into(),
        );
    }
    Ok(())
}

/// A route as a router might match it: percent escapes decoded, runs of
/// `/` made one, a trailing `/` and any fragment dropped, in lower case.
fn normalized_route(route: &str) -> String {
    let route = route.split('#').next().unwrap_or(route);
    let bytes = route.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let hex = |at: usize| bytes.get(at).and_then(|byte| (*byte as char).to_digit(16));
        match (bytes[index], hex(index + 1), hex(index + 2)) {
            (b'%', Some(high), Some(low)) => {
                decoded.push((high * 16 + low) as u8);
                index += 3;
            }
            (byte, _, _) => {
                decoded.push(byte);
                index += 1;
            }
        }
    }
    let decoded = String::from_utf8_lossy(&decoded).to_lowercase();
    let mut out = String::with_capacity(decoded.len());
    for ch in decoded.chars() {
        if !(ch == '/' && out.ends_with('/')) {
            out.push(ch);
        }
    }
    while out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

/// Readable routes, for `api_routes`: the path pattern and what it answers.
/// A test checks each pattern against the router's own route list.
pub(super) const READABLE_ROUTES: &[(&str, &str)] = &[
    ("/info", "Server version, database management, banner"),
    ("/databases", "Open catalogs with their image folders"),
    (
        "/stack-activity",
        "Stack builds running and queued across databases",
    ),
    ("/wbpp/activity", "WBPP runs running and queued"),
    (
        "/settings/calibration",
        "Calibration matching settings, global and per rig, with night_zone (where a rig's nights split)",
    ),
    ("/settings/stacking", "Stack automation settings"),
    ("/settings/stacking/method", "Default stacking method"),
    (
        "/settings/storage",
        "Where generated files live, and volume limits",
    ),
    ("/settings/export", "Export and WBPP defaults"),
    ("/settings/pixinsight", "PixInsight install and run folder"),
    (
        "/settings/workers",
        "Processor shares for interactive and background work",
    ),
    ("/settings/astrobin", "AstroBin export settings"),
    ("/peers", "Configured sync peers"),
    (
        "/astrometry/capabilities",
        "Plate-solving resources installed",
    ),
    ("/processing-setups", "Saved stack processing setups"),
    ("/update-notice", "Newer release notice"),
    ("/db/{db}/projects", "Projects (plain list)"),
    (
        "/db/{db}/projects/overview",
        "Projects with targets, plans, progress",
    ),
    (
        "/db/{db}/projects/{project_id}/targets",
        "A project's targets",
    ),
    (
        "/db/{db}/projects/{project_id}/scheduler",
        "A project's Target Scheduler settings, targets and exposure plans",
    ),
    (
        "/db/{db}/projects/{project_id}/processing-settings",
        "A project's stack processing settings",
    ),
    (
        "/db/{db}/projects/{project_id}/mosaic",
        "A project's mosaic panels",
    ),
    (
        "/db/{db}/projects/{project_id}/calibration-report",
        "Calibration coverage by night and filter; ?include_rejected",
    ),
    (
        "/db/{db}/projects/{project_id}/calibration-report/rejects",
        "Lights the library cannot calibrate, with a digest",
    ),
    (
        "/db/{db}/projects/{project_id}/stack-previews/latest",
        "Latest stack per channel",
    ),
    (
        "/db/{db}/projects/{project_id}/stack-previews/{job_id}",
        "One stack job with frame decisions",
    ),
    (
        "/db/{db}/projects/{project_id}/stack-previews/color",
        "Colour compositions available",
    ),
    (
        "/db/{db}/projects/{project_id}/stack-previews/wbpp",
        "Stacks made by WBPP",
    ),
    ("/db/{db}/targets", "All targets"),
    (
        "/db/{db}/targets/overview",
        "Targets with grade counts and last capture",
    ),
    (
        "/db/{db}/images",
        "Lights; ?project_id&target_id&status&filter_name&limit&offset",
    ),
    ("/db/{db}/images/{image_id}", "One light"),
    (
        "/db/{db}/images/{image_id}/astrometry",
        "Stored plate solve",
    ),
    (
        "/db/{db}/images/{image_id}/satellites",
        "Stored satellite predictions",
    ),
    ("/db/{db}/images/{image_id}/stars", "Detected stars"),
    (
        "/db/{db}/images/{image_id}/calibration",
        "Why the light got, or missed, each master",
    ),
    (
        "/db/{db}/analysis/image/{image_id}",
        "Quality score and issues",
    ),
    (
        "/db/{db}/analysis/sequence",
        "Relative scores; ?target_id&project_id&all_projects&filter_name",
    ),
    (
        "/db/{db}/analysis/quality-backfill",
        "Quality scan progress",
    ),
    (
        "/db/{db}/analysis/quality-scan",
        "Quality scan progress",
    ),
    ("/db/{db}/import", "Import progress"),
    ("/db/{db}/import/folders", "Image folders to import from"),
    ("/db/{db}/autoimport", "Automatic import status"),
    ("/db/{db}/wbpp/runs/current", "The current WBPP run"),
    ("/db/{db}/stats/overall", "Counts by grade, exposure"),
    (
        "/db/{db}/sky/coverage",
        "Each target's footprint and exposure by filter",
    ),
    ("/db/{db}/calibrations", "Calibration totals per rig"),
    (
        "/db/{db}/calibrations/details",
        "Calibration frames in detail, with grades",
    ),
    (
        "/db/{db}/calibrations/quality",
        "Calibration grading progress, and frames by grade",
    ),
    (
        "/db/{db}/calibrations/frames",
        "Calibration frames a page at a time; ?rig&kind&filter&exposure&temp_min&temp_max&night_from&night_to&grade&kept_out&rotation&session&project&group&sort&page&per_page",
    ),
    (
        "/db/{db}/calibrations/sessions",
        "Calibration sessions (night, kind, settings) with counts by grade, and the values the filters take; ?project and the frame filters",
    ),
    (
        "/db/{db}/calibrations/project-scope",
        "What one project's lights use from the library: sessions, masters, rigs; ?project",
    ),
    (
        "/db/{db}/calibrations/frames/{frame_uuid}",
        "One calibration frame: measurement, findings, its session's numbers",
    ),
    (
        "/db/{db}/calibrations/masters",
        "Recorded calibration masters: sources, statistics, projects using them",
    ),
    (
        "/db/{db}/calibrations/flat-groups",
        "Flat groups per rig: families, groups, splits and joins (measured or by hand, with any conflict), change events with their dust and vignetting, rig clocks; ?project",
    ),
    (
        "/db/{db}/calibrations/flat-groups/use",
        "What each flat group feeds: its masters and the lights that take it; ?group",
    ),
    (
        "/db/{db}/calibrations/flat-groups/place",
        "The family, night and angle a flat session would take among the groups; ?session",
    ),
    (
        "/db/{db}/calibrations/flat-measure",
        "Flat measuring progress: rig, stage, retry, and whether it is paused",
    ),
    (
        "/db/{db}/jobs/paused",
        "The jobs a person paused for the database: quality, pregeneration, flats, sites",
    ),
    (
        "/db/{db}/calibrations/frame-sites",
        "Progress reading where older calibration frames were shot",
    ),
    (
        "/db/{db}/calibrations/duplicates",
        "The duplicate check or repair, and its plan: exposures recorded more than once, what a repair would merge, its token",
    ),
    (
        "/db/{db}/calibration-health",
        "Each project's calibration health: poor or check, with light counts",
    ),
    (
        "/db/{db}/projects/{project_id}/calibration-health",
        "One project's calibration findings by night and filter",
    ),
    (
        "/db/{db}/calibration-health/lights",
        "Lights with a calibration finding: level and codes by image id; ?project_id",
    ),
    ("/db/{db}/flat-history", "Target Scheduler flat history"),
    ("/db/{db}/rejects/removed", "Removed rejects, by batch"),
    (
        "/databases/{db}/sync/previews",
        "Sync previews of a database",
    ),
    (
        "/db/{db}/astrobin-export",
        "AstroBin acquisition rows; ?project_id&target_id&include_pending&detail",
    ),
    ("/db/{db}/astrobin/filters", "AstroBin filter map"),
    (
        "/db/{db}/calibrated-copies",
        "Calibrated-copy pairing settings",
    ),
    ("/db/{db}/guids", "Rows missing GUIDs"),
    (
        "/db/{db}/renamed-lights",
        "Lights catalogued twice under a renamed file",
    ),
    ("/director/v1/status", "Whether Planning is on"),
    ("/director/v1/plans", "Plans across rigs"),
    ("/director/v1/projects", "Planning projects"),
    ("/director/v1/projects/{plan_id}", "One planning project"),
    (
        "/director/v1/projects/{plan_id}/plan",
        "A plan: goals, rigs, exposures",
    ),
    (
        "/director/v1/projects/{plan_id}/plan/progress",
        "Frames per rig and objective",
    ),
    (
        "/director/v1/projects/{plan_id}/framing",
        "The framing draft",
    ),
    (
        "/director/v1/projects/{plan_id}/mosaic",
        "Mosaic panels and their stacks",
    ),
    (
        "/director/v1/projects/{plan_id}/activation",
        "The last activation",
    ),
    (
        "/director/v1/projects/{plan_id}/activation/check",
        "Whether the plan differs from what was activated",
    ),
    ("/director/v1/rigs/profiles", "Rig profiles"),
    ("/director/v1/rigs/status", "Live rig status"),
    (
        "/director/v1/rigs/{rig}/preferences",
        "Effective observing preferences; ?project_id",
    ),
    (
        "/director/v1/rigs/{rig}/scheduling",
        "Scheduling limits a rig would apply, per project",
    ),
    (
        "/director/v1/catalogs/{db}/templates",
        "A rig catalog's exposure templates",
    ),
    (
        "/director/v1/catalogs/{db}/rig/profile",
        "A catalog's rig profile",
    ),
    ("/director/v1/templates", "The exposure template library"),
    (
        "/director/v1/preferences",
        "Observing preference defaults and scopes",
    ),
    (
        "/director/v1/sky/search",
        "Find a target by name; ?q&limit&online",
    ),
    (
        "/director/v1/sky/resolve",
        "Resolve a name to coordinates; ?name",
    ),
    (
        "/director/v1/sky/surveys",
        "Survey images available for framing",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Every route `api_routes` names reaches a handler: an unmatched path
    /// gets the router's own empty 404, a matched one a JSON answer (here
    /// mostly 401, since the test server trusts no anonymous caller).
    #[tokio::test]
    async fn every_listed_route_exists() {
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        let router = crate::server::api_router(Arc::clone(&state));
        let uuid = "00000000-0000-0000-0000-000000000001";
        let job = "a".repeat(64);
        let mut missing = Vec::new();
        for (pattern, _) in READABLE_ROUTES {
            let path = pattern
                .replace("{db}", "test")
                .replace("{project_id}", "1")
                .replace("{image_id}", "1")
                .replace("{job_id}", &job)
                .replace("{plan_id}", uuid)
                .replace("{frame_uuid}", uuid)
                .replace("{rig}", uuid);
            let request = Request::get(&path).body(Body::empty()).unwrap();
            let response = router.clone().oneshot(request).await.unwrap();
            let status = response.status();
            let body = axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap();
            if status == StatusCode::NOT_FOUND && body.is_empty() {
                missing.push(*pattern);
            }
        }
        assert!(missing.is_empty(), "not routes: {missing:?}");
    }

    #[test]
    fn only_paths_below_the_api_are_read() {
        assert!(readable_path("/db/test/images").is_ok());
        assert!(readable_path("db/test/images").is_err());
        assert!(readable_path("/mcp").is_err());
        assert!(readable_path("/mcp/anything").is_err());
        assert!(readable_path("/db/../settings").is_err());
        for backup in [
            "/backups",
            "/backups/status",
            "/backups/files/downloads/psf-guard-backup.zip",
            "//backups/files/downloads/a.zip",
            "/Backups/files/downloads/a.zip",
            "/%62ackups/files/downloads/a.zip",
            "/backups%2Ffiles%2Fdownloads%2Fa.zip",
            "/backups/?x=1",
            "/db/%2e%2e/backups",
        ] {
            assert!(readable_path(backup).is_err(), "{backup}");
        }
        assert!(readable_path("/db/test/backupsish").is_ok());
        assert_eq!(
            with_query("/x", [("q", Some("NGC 7635".into())), ("skip", None)]),
            "/x?q=NGC%207635"
        );
    }
}
