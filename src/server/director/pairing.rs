//! Separate Director credentials; never Sync keys or global operator tokens.
use super::*;
use crate::{
    auth_registry::{hash_token, AccessRole},
    server::auth::RequestAccess,
};
use axum::{
    http::{
        header::{AUTHORIZATION, CACHE_CONTROL},
        HeaderMap, Method,
    },
    Extension,
};
use psf_guard_director_meta::client::Client;

const PAIR_PREFIX: &str = "psfdpt_";
const CLIENT_PREFIX: &str = "psfdrc_";
const PROFILE_HEADER: &str = "x-psf-director-profile";
const SCOPES: [&str; 3] = ["program:read", "checkin:write", "status:write"];

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rigs/{rig}/pairing-token", axum::routing::post(issue))
        .route("/pair", axum::routing::post(pair))
        .route("/rigs/{rig}/clients", get(clients))
        .route(
            "/rigs/{rig}/clients/{client}",
            axum::routing::delete(revoke),
        )
}

fn denied() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(CACHE_CONTROL, "no-store")],
        Json(ApiResponse::<()>::error(
            "Invalid Director client credentials or scope".into(),
        )),
    )
        .into_response()
}

fn no_store(response: impl IntoResponse) -> Response {
    let mut response = response.into_response();
    response
        .headers_mut()
        .insert(CACHE_CONTROL, "no-store".parse().unwrap());
    response
}

/// Called before anonymous-loopback or cookie fallbacks. `true` means this
/// request has its own narrowly scoped authorization, not operator authority.
pub(in crate::server) async fn authorize(
    state: &Arc<AppState>,
    path: &str,
    method: &Method,
    headers: &HeaderMap,
) -> Result<bool, Response> {
    let director_path = path.starts_with("/director/v1/");
    let authorization = headers.get(AUTHORIZATION);
    let raw = authorization.and_then(|v| v.to_str().ok());
    let secret = raw
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, secret)| secret.trim());
    let director_token =
        secret.is_some_and(|s| s.starts_with(CLIENT_PREFIX) || s.starts_with(PAIR_PREFIX));
    if !director_path && !director_token {
        return Ok(false);
    }
    if headers.get_all(AUTHORIZATION).iter().count() > 1 {
        return Err(denied());
    }
    if path == "/director/v1/pair" && method == Method::POST {
        return if authorization.is_none() {
            Ok(true)
        } else {
            Err(denied())
        };
    }
    if !director_token {
        // Preserve ordinary operator sessions/PATs, but an unrelated bearer
        // (including Sync) never falls through to an operator cookie or loopback.
        return if authorization.is_some()
            && (!secret.is_some_and(|s| s.starts_with("psfg_")) || state.server_auth().is_none())
        {
            Err(denied())
        } else {
            Ok(false)
        };
    }
    let secret = secret
        .filter(|s| valid_secret(s, CLIENT_PREFIX))
        .ok_or_else(denied)?;
    let segments: Vec<_> = path.split('/').collect();
    let rig = match segments.as_slice() {
        ["", "director", "v1", "rigs", rig, "program" | "allocation"] if method == Method::GET => {
            *rig
        }
        ["", "director", "v1", "rigs", rig, "checkin" | "status"] if method == Method::POST => *rig,
        ["", "director", "v1", "rigs", rig, "allocation", "start"] if method == Method::POST => {
            *rig
        }
        _ => return Err(denied()),
    };
    let rig = Uuid::parse_str(rig)
        .ok()
        .filter(|id| id.to_string() == rig)
        .ok_or_else(denied)?;
    if headers.get_all(PROFILE_HEADER).iter().count() != 1 {
        return Err(denied());
    }
    let profile = headers
        .get(PROFILE_HEADER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok().filter(|id| id.to_string() == s))
        .ok_or_else(denied)?;
    let service = enabled(state).map_err(|e| no_store(e.into_response()))?;
    let hash = hash_token(secret);
    let client = service
        .clone()
        .query(move |store| store.client_for_token(&hash))
        .await
        .map_err(|e| no_store(e.into_response()))?
        .ok_or_else(denied)?;
    if client.rig_id != rig || client.profile_id != profile {
        return Err(denied());
    }
    // Existing credentials gain no blanket allocation capability. Only the
    // exact client explicitly selected by an operator may fetch its grant.
    if path.ends_with("/allocation") || path.ends_with("/allocation/start") {
        let allocation = service
            .query(move |store| store.allocation(rig))
            .await
            .map_err(|e| no_store(e.into_response()))?
            .ok_or_else(denied)?;
        if allocation.client_id != client.client_id
            || allocation.profile_id != profile
            || allocation.catalog_id != client.catalog_id
        {
            return Err(denied());
        }
    }
    Ok(true)
}

pub(super) fn operator(access: &RequestAccess) -> Result<(), Box<Response>> {
    if access.api_token || access.role != AccessRole::ReadWrite {
        return Err(Box::new(no_store((
            StatusCode::FORBIDDEN,
            Json(ApiResponse::<()>::error(
                "Director client management requires an interactive editor session".into(),
            )),
        ))));
    }
    Ok(())
}

fn now_ms() -> Result<i64, Error> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .ok_or(Error::Internal)
}

fn mint(prefix: &str) -> String {
    use std::fmt::Write;
    let mut secret = String::with_capacity(prefix.len() + 64);
    secret.push_str(prefix);
    for byte in rand::random::<[u8; 32]>() {
        write!(&mut secret, "{byte:02x}").unwrap();
    }
    secret
}
fn valid_secret(secret: &str, prefix: &str) -> bool {
    secret.strip_prefix(prefix).is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Issue {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
}

async fn issue(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Extension(access): Extension<RequestAccess>,
    Json(input): Json<Issue>,
) -> Response {
    async {
        operator(&access).map_err(|response| *response)?;
        let service = enabled(&state).map_err(IntoResponse::into_response)?;
        if input.coordinator_instance_id != service.instance_id {
            return Err(Error::WrongRig.into_response());
        }
        let secret = mint(PAIR_PREFIX);
        let hash = hash_token(&secret);
        let now = now_ms().map_err(IntoResponse::into_response)?;
        let expires = now
            .checked_add(3_600_000)
            .ok_or_else(|| Error::Internal.into_response())?;
        service
            .run(move |store| {
                store.issue_client_pairing(input.catalog_id, rig, &hash, now, expires)
            })
            .await
            .map_err(IntoResponse::into_response)?;
        Ok(Json(ApiResponse::success(
            serde_json::json!({"protocol_version":1,
            "coordinator_instance_id":input.coordinator_instance_id,"catalog_id":input.catalog_id,
            "rig_id":rig,"pairing_token":secret,"expires_at_ms":expires}),
        )))
    }
    .await
    .map_or_else(no_store, no_store)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Pair {
    protocol_version: u32,
    pairing_token: String,
    profile_id: Uuid,
    client_name: String,
}

async fn pair(
    State(state): State<Arc<AppState>>,
    input: Result<Json<Pair>, axum::extract::rejection::JsonRejection>,
) -> Response {
    // Serde may quote a malformed field/value. Never reflect a pairing secret
    // accidentally pasted into the wrong field, even on extraction failures.
    let Json(input) = match input {
        Ok(input) => input,
        Err(error) => {
            return no_store((
                error.status(),
                Json(ApiResponse::<()>::error(
                    "Invalid Director pairing request".into(),
                )),
            ))
        }
    };
    async {
        let service = enabled(&state).map_err(IntoResponse::into_response)?;
        if input.protocol_version != 1
            || !valid_secret(&input.pairing_token, PAIR_PREFIX)
            || input.profile_id.is_nil()
            || input.client_name.is_empty()
            || input.client_name.len() > 80
            || input.client_name.trim() != input.client_name
            || input.client_name.chars().any(char::is_control)
        {
            return Err(denied());
        }
        let instance = service.instance_id;
        let secret = mint(CLIENT_PREFIX);
        let token_hash = hash_token(&secret);
        let pairing_hash = hash_token(&input.pairing_token);
        let now = now_ms().map_err(IntoResponse::into_response)?;
        let client = service
            .run(move |store| {
                store.redeem_client_pairing(
                    &pairing_hash,
                    &token_hash,
                    input.profile_id,
                    &input.client_name,
                    now,
                )
            })
            .await
            .map_err(|e| match e {
                Error::Missing | Error::Invalid | Error::Conflict => denied(),
                other => other.into_response(),
            })?;
        Ok(Json(ApiResponse::success(serde_json::json!({
            "protocol_version": 1,
            "coordinator_instance_id": instance,
            "catalog_id": client.catalog_id,
            "rig_id": client.rig_id,
            "profile_id": client.profile_id,
            "client_id": client.client_id,
            "token": secret,
            "scopes": SCOPES
        }))))
    }
    .await
    .map_or_else(no_store, no_store)
}

async fn clients(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Extension(access): Extension<RequestAccess>,
) -> Response {
    async {
        operator(&access).map_err(|response| *response)?;
        let service = enabled(&state).map_err(IntoResponse::into_response)?;
        let clients: Vec<Client> = service
            .query(move |store| store.clients(rig))
            .await
            .map_err(IntoResponse::into_response)?;
        Ok::<_, Response>(Json(ApiResponse::success(clients)))
    }
    .await
    .map_or_else(no_store, no_store)
}

async fn revoke(
    State(state): State<Arc<AppState>>,
    Path((rig, client)): Path<(Uuid, Uuid)>,
    Extension(access): Extension<RequestAccess>,
) -> Response {
    async {
        operator(&access).map_err(|response| *response)?;
        let service = enabled(&state).map_err(IntoResponse::into_response)?;
        let revoked = service
            .run(move |store| store.revoke_client(rig, client))
            .await
            .map_err(IntoResponse::into_response)?;
        Ok::<_, Response>(Json(ApiResponse::success(
            serde_json::json!({"revoked":revoked}),
        )))
    }
    .await
    .map_or_else(no_store, no_store)
}
