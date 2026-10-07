//! Host-owned collaboration authentication. No acquisition or token ingress API.
use super::*;
use crate::server::auth::{self, RequestAccess};
use axum::{
    http::{
        header::{HOST, ORIGIN},
        HeaderMap,
    },
    Extension,
};
use psf_guard_director_interop::{
    astrocollab::{self, Source},
    auth as wire,
};
use psf_guard_director_meta::collaboration_connection::{ConnectionBinding, ConnectionState};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant},
};
mod credentials;
#[cfg(test)]
mod tests;

#[derive(Default)]
pub(super) struct SessionState {
    gate: tokio::sync::Mutex<()>,
    pending: Mutex<HashMap<Uuid, Pending>>,
}
struct Pending {
    owner: String,
    code: String,
    expires: Instant,
    next_poll: Instant,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    id: Uuid,
    server_url: String,
    name: String,
    #[serde(default)]
    allow_loopback_http: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    code: Option<String>,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Action {
    Discover,
    Pair,
    Signin,
    Poll,
    Cancel,
    Disconnect,
    Validate,
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rigs/{rig}/collaboration", get(list).post(create))
        .route("/collaboration/{id}/{action}", axum::routing::post(action))
}
pub(super) fn credential_path(registry: &FilePath) -> PathBuf {
    credentials::path(registry)
}
#[derive(Debug)]
struct Failure(StatusCode, &'static str);
impl From<Error> for Failure {
    fn from(e: Error) -> Self {
        match e {
            Error::Missing => Self(StatusCode::NOT_FOUND, "Collaboration binding not found"),
            Error::Invalid => Self(StatusCode::BAD_REQUEST, "Invalid collaboration request"),
            Error::Conflict => Self(
                StatusCode::CONFLICT,
                "Collaboration binding changed; reload before retrying",
            ),
            Error::Forbidden | Error::Disabled => Self(
                StatusCode::FORBIDDEN,
                "Collaboration setup requires database management and Director",
            ),
            _ => Self(
                StatusCode::SERVICE_UNAVAILABLE,
                "Collaboration metadata unavailable",
            ),
        }
    }
}
impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        // A remote rejection is not an expired local browser session.
        let status = if self.0 == StatusCode::UNAUTHORIZED {
            StatusCode::CONFLICT
        } else {
            self.0
        };
        pairing::no_store((status, Json(ApiResponse::<()>::error(self.1.into()))))
    }
}
fn storage_error(_: std::io::Error) -> Failure {
    Failure(StatusCode::SERVICE_UNAVAILABLE, "Collaboration credentials unavailable; check config file ownership, permissions and writable storage")
}
fn invalid() -> Failure {
    Failure(
        StatusCode::BAD_REQUEST,
        "Invalid collaboration request or server reply",
    )
}
fn conflict() -> Failure {
    Failure(StatusCode::CONFLICT, "This agent is already registered or setup has an uncertain outcome; retain this binding and register a new connection explicitly")
}
fn registry(state: &AppState) -> Result<PathBuf, Failure> {
    state
        .registry_path
        .read()
        .map_err(|_| invalid())?
        .as_deref()
        .map(credentials::path)
        .ok_or(Failure(
            StatusCode::FORBIDDEN,
            "Collaboration setup needs a persistent config registry",
        ))
}
fn operator(
    state: &AppState,
    access: &RequestAccess,
    headers: &HeaderMap,
) -> Result<String, Failure> {
    if !state.database_management_allowed()
        || access.api_token
        || access.role != auth::AccessRole::ReadWrite
    {
        return Err(Failure(
            StatusCode::FORBIDDEN,
            "Collaboration setup requires an interactive editor and database management",
        ));
    }
    if let Some(origin) = headers.get(ORIGIN) {
        let origin: reqwest::Url = origin
            .to_str()
            .ok()
            .and_then(|s| s.parse().ok())
            .ok_or_else(invalid)?;
        let host = headers
            .get(HOST)
            .and_then(|h| h.to_str().ok())
            .ok_or_else(invalid)?;
        let expected =
            reqwest::Url::parse(&format!("{}://{host}", origin.scheme())).map_err(|_| invalid())?;
        let tauri = state.anonymous_access_trusted()
            && matches!(
                origin.as_str(),
                "http://tauri.localhost/" | "https://tauri.localhost/" | "tauri://localhost"
            );
        if !tauri
            && (origin.origin() != expected.origin()
                || !matches!(origin.scheme(), "http" | "https"))
        {
            return Err(Failure(
                StatusCode::FORBIDDEN,
                "Collaboration setup requires a same-origin request",
            ));
        }
    }
    auth::setup_owner(state, headers).ok_or(Failure(
        StatusCode::FORBIDDEN,
        "Sign in again before collaboration setup",
    ))
}
async fn file<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, Failure> {
    blocking(work).await?.map_err(storage_error)
}
async fn binding(service: Arc<Service>, id: Uuid) -> Result<ConnectionBinding, Failure> {
    service
        .query(move |s| s.collaboration_connection(id))
        .await?
        .ok_or(Error::Missing.into())
}
async fn change(
    service: Arc<Service>,
    old: ConnectionBinding,
    state: ConnectionState,
    agent: Option<String>,
) -> Result<ConnectionBinding, Failure> {
    let mut new = old.clone();
    new.state = state;
    if agent.is_some() {
        new.agent_id = agent;
    }
    service
        .run(move |s| {
            s.update_collaboration_connection(&old, &new)?;
            Ok(new)
        })
        .await
        .map_err(Into::into)
}
async fn view(path: PathBuf, b: ConnectionBinding) -> Result<Value, Failure> {
    let copy = b.clone();
    let status = match file(move || credentials::read(&path, &copy)).await {
        Err(_) => "storage_unavailable",
        Ok(_) if b.state == ConnectionState::Disabled => "disconnected",
        Ok(None) if b.agent_id.is_some() => "credential_missing",
        Ok(Some(_)) if b.state == ConnectionState::Rejected => "reauth_required",
        Ok(Some(_)) => "registered",
        Ok(None) if b.state == ConnectionState::OutcomeUnknown => "outcome_unknown",
        Ok(None) => "not_connected",
    };
    Ok(json!({"binding": b, "status": status}))
}
async fn list(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
) -> Result<Json<ApiResponse<Vec<Value>>>, Failure> {
    let service = enabled(&state)?;
    let p = registry(&state)?;
    let entries = service
        .clone()
        .query(move |s| s.collaboration_connections(rig))
        .await?;
    let mut result = Vec::new();
    for b in entries {
        let id = b.id;
        let mut row = view(p.clone(), b).await?;
        if service
            .collaboration
            .pending
            .lock()
            .map_err(|_| invalid())?
            .get(&id)
            .is_some_and(|p| p.expires > Instant::now())
        {
            row["status"] = json!("awaiting_browser");
        }
        result.push(row);
    }
    Ok(Json(ApiResponse::success(result)))
}
async fn create(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Extension(access): Extension<RequestAccess>,
    headers: HeaderMap,
    Json(input): Json<Create>,
) -> Result<Json<ApiResponse<Value>>, Failure> {
    operator(&state, &access, &headers)?;
    let service = writable(&state)?;
    let source = Source::new(&input.server_url, "000000000000", input.allow_loopback_http)
        .map_err(|_| invalid())?;
    let b = ConnectionBinding {
        id: input.id,
        rig_id: rig,
        base_url: source.base_url().into(),
        name: input.name,
        allow_loopback_http: input.allow_loopback_http,
        agent_id: None,
        state: ConnectionState::New,
    };
    let p = registry(&state)?;
    let path = p.clone();
    file(move || credentials::available(&path)).await?;
    let copy = b.clone();
    service
        .run(move |s| s.create_collaboration_connection(&copy))
        .await?;
    Ok(Json(ApiResponse::success(view(p, b).await?)))
}

async fn action(
    State(state): State<Arc<AppState>>,
    Path((id, action)): Path<(Uuid, Action)>,
    Extension(access): Extension<RequestAccess>,
    headers: HeaderMap,
    Json(input): Json<Input>,
) -> Response {
    let owner = match operator(&state, &access, &headers) {
        Ok(owner) => owner,
        Err(e) => return e.into_response(),
    };
    let service = match writable(&state) {
        Ok(s) => s,
        Err(e) => return Failure::from(e).into_response(),
    };
    // Token-producing requests finish persistence even if the browser goes away.
    match tokio::spawn(async move {
        let _guard = service.collaboration.gate.try_lock().map_err(|_| {
            Failure(
                StatusCode::SERVICE_UNAVAILABLE,
                "Collaboration setup is busy",
            )
        })?;
        execute(&state, service.clone(), id, action, input, owner).await
    })
    .await
    {
        Ok(Ok(value)) => pairing::no_store(Json(ApiResponse::success(value))),
        Ok(Err(e)) => e.into_response(),
        Err(_) => Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Collaboration setup did not complete; reload its state before retrying",
        )
        .into_response(),
    }
}
fn pending_remove(service: &Service, id: Uuid) -> Result<Option<Pending>, Failure> {
    Ok(service
        .collaboration
        .pending
        .lock()
        .map_err(|_| invalid())?
        .remove(&id))
}
fn pending_insert(service: &Service, id: Uuid, p: Pending) -> Result<(), Failure> {
    let mut pending = service
        .collaboration
        .pending
        .lock()
        .map_err(|_| invalid())?;
    pending.retain(|_, p| p.expires > Instant::now());
    if pending.len() >= 32 {
        return Err(Failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "Too many pending sign-ins",
        ));
    }
    pending.insert(id, p);
    Ok(())
}
async fn execute(
    state: &AppState,
    service: Arc<Service>,
    id: Uuid,
    action: Action,
    input: Input,
    owner: String,
) -> Result<Value, Failure> {
    let mut b = binding(service.clone(), id).await?;
    let p = registry(state)?;
    let remote = Remote::new(&b)?;
    if input.code.is_some() && !matches!(action, Action::Pair) {
        return Err(invalid());
    }
    match action {
        Action::Discover => return remote.discover().await,
        Action::Disconnect => {
            pending_remove(&service, id)?;
            b = change(service, b, ConnectionState::Disabled, None).await?;
            let copy = b.clone();
            let path = p.clone();
            file(move || credentials::write(&path, &copy, None)).await?;
        }
        Action::Cancel => {
            let pending = pending_remove(&service, id)?.ok_or_else(conflict)?;
            if pending.owner != owner {
                pending_insert(&service, id, pending)?;
                return Err(Failure(
                    StatusCode::FORBIDDEN,
                    "Sign-in belongs to another session",
                ));
            }
            b = change(service, b, ConnectionState::New, None).await?;
        }
        Action::Validate => {
            if b.state == ConnectionState::Disabled {
                return Err(conflict());
            }
            let copy = b.clone();
            let path = p.clone();
            let token = file(move || credentials::read(&path, &copy))
                .await?
                .ok_or(Failure(
                    StatusCode::CONFLICT,
                    "Credential missing; the original agent binding and reports are retained",
                ))?;
            match remote.get("agent/projects", Some(&token), None).await {
                Err(Failure(StatusCode::UNAUTHORIZED, msg)) => {
                    change(service, b, ConnectionState::Rejected, None).await?;
                    return Err(Failure(StatusCode::UNAUTHORIZED, msg));
                }
                Err(e) => return Err(e),
                Ok(bytes) => {
                    astrocollab::decode_projects(&bytes, &b.source().map_err(Error::from)?)
                        .map_err(|_| invalid())?;
                }
            }
            b = change(service, b, ConnectionState::Registered, None).await?;
        }
        Action::Pair | Action::Signin => {
            if b.agent_id.is_some() || b.state != ConnectionState::New {
                return Err(conflict());
            }
            {
                let mut pending = service
                    .collaboration
                    .pending
                    .lock()
                    .map_err(|_| invalid())?;
                pending.retain(|_, p| p.expires > Instant::now());
                if pending.contains_key(&id) {
                    return Err(Failure(
                        StatusCode::CONFLICT,
                        "Browser sign-in is already pending; finish or cancel it first",
                    ));
                }
            }
            let path = p.clone();
            file(move || credentials::available(&path)).await?;
            let capabilities = remote.discover().await?;
            if matches!(action, Action::Pair) {
                if capabilities["pairing"] != true {
                    return Err(Failure(
                        StatusCode::CONFLICT,
                        "This server does not advertise pairing",
                    ));
                }
                let code = input
                    .code
                    .filter(|s| !s.is_empty() && s.len() <= 512 && !s.chars().any(char::is_control))
                    .ok_or_else(invalid)?;
                b = change(service.clone(), b, ConnectionState::OutcomeUnknown, None).await?;
                let bytes = request_enrollment(
                    &remote,
                    service.clone(),
                    &b,
                    "pair",
                    None,
                    json!({"code":code,"name":b.name}),
                )
                .await?;
                b = persist_enrollment(service.clone(), p.clone(), b, &bytes).await?;
            } else {
                if capabilities["signin"] != true {
                    return Err(Failure(
                        StatusCode::CONFLICT,
                        "This server does not offer browser sign-in",
                    ));
                }
                let bytes = remote.post("auth/login", None, json!({})).await?;
                let login = wire::login(&bytes).map_err(|_| invalid())?;
                let url = remote.browser_url(&login.url)?;
                pending_insert(
                    &service,
                    id,
                    Pending {
                        owner,
                        code: login.code,
                        expires: Instant::now() + Duration::from_secs(login.expires_in),
                        next_poll: Instant::now() + Duration::from_secs(2),
                    },
                )?;
                return Ok(
                    json!({"status":"awaiting_browser", "url":url, "expires_in":login.expires_in}),
                );
            }
        }
        Action::Poll => {
            let mut pending = pending_remove(&service, id)?.ok_or(Failure(
                StatusCode::CONFLICT,
                "Sign-in expired or was lost after restart; start browser sign-in again",
            ))?;
            if pending.owner != owner {
                pending_insert(&service, id, pending)?;
                return Err(Failure(
                    StatusCode::FORBIDDEN,
                    "Sign-in belongs to another session",
                ));
            }
            if pending.expires <= Instant::now() {
                return Err(Failure(StatusCode::CONFLICT, "Sign-in expired"));
            }
            if pending.next_poll > Instant::now() {
                pending_insert(&service, id, pending)?;
                return Err(Failure(
                    StatusCode::TOO_MANY_REQUESTS,
                    "Wait before checking browser sign-in again",
                ));
            }
            if b.agent_id.is_some() || b.state != ConnectionState::New {
                return Err(conflict());
            }
            let path = p.clone();
            if let Err(error) = file(move || credentials::available(&path)).await {
                pending_insert(&service, id, pending)?;
                return Err(error);
            }
            let bytes = remote
                .get("auth/poll", None, Some(("code", &pending.code)))
                .await?;
            match wire::poll(&bytes).map_err(|_| invalid())? {
                wire::Poll::Pending => { pending.next_poll = Instant::now() + Duration::from_secs(2); pending_insert(&service, id, pending)?; return Ok(json!({"status":"awaiting_browser"})); }
                wire::Poll::Expired | wire::Poll::Claimed => return Err(Failure(StatusCode::CONFLICT, "Sign-in expired or its one-time reply was already claimed; start a new sign-in")),
                wire::Poll::Done(person) => {
                    b = change(service.clone(), b, ConnectionState::OutcomeUnknown, None).await?;
                    let bytes = request_enrollment(&remote, service.clone(), &b, "agents", Some(&person), json!({"name":b.name})).await?;
                    b = persist_enrollment(service.clone(), p.clone(), b, &bytes).await?;
                }
            }
        }
    }
    view(p, b).await
}
async fn request_enrollment(
    remote: &Remote,
    service: Arc<Service>,
    b: &ConnectionBinding,
    route: &str,
    token: Option<&str>,
    body: Value,
) -> Result<Vec<u8>, Failure> {
    match remote.post(route, token, body).await {
        Err(error)
            if matches!(
                error.0,
                StatusCode::UNAUTHORIZED
                    | StatusCode::CONFLICT
                    | StatusCode::UNPROCESSABLE_ENTITY
                    | StatusCode::TOO_MANY_REQUESTS
            ) =>
        {
            change(service, b.clone(), ConnectionState::New, None).await?;
            if error.0 == StatusCode::UNAUTHORIZED {
                return Err(Failure(
                    StatusCode::CONFLICT,
                    if route == "pair" {
                        "Pairing code rejected; enter a fresh code"
                    } else {
                        "Sign-in expired; start browser sign-in again"
                    },
                ));
            }
            Err(error)
        }
        other => other,
    }
}
async fn persist_enrollment(
    service: Arc<Service>,
    path: PathBuf,
    b: ConnectionBinding,
    bytes: &[u8],
) -> Result<ConnectionBinding, Failure> {
    let enrolled = wire::enrollment(bytes).map_err(|_| invalid())?;
    let b = change(
        service,
        b,
        ConnectionState::Registered,
        Some(enrolled.agent_id),
    )
    .await?;
    let copy = b.clone();
    file(move || credentials::write(&path, &copy, Some(&enrolled.token))).await?;
    Ok(b)
}

struct Remote {
    base: reqwest::Url,
    client: reqwest::Client,
}
impl Remote {
    fn new(b: &ConnectionBinding) -> Result<Self, Failure> {
        let base = reqwest::Url::parse(b.source().map_err(Error::from)?.base_url())
            .map_err(|_| invalid())?;
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .referer(false)
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| invalid())?;
        Ok(Self { base, client })
    }
    fn browser_url(&self, value: &str) -> Result<String, Failure> {
        let url: reqwest::Url = value.parse().map_err(|_| invalid())?;
        if url.origin() != self.base.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(Failure(
                StatusCode::BAD_GATEWAY,
                "Sign-in URL does not match the configured server; check its public URL",
            ));
        }
        Ok(url.to_string())
    }
    async fn discover(&self) -> Result<Value, Failure> {
        let health = astrocollab::decode_health(&self.get("health", None, None).await?)
            .map_err(|_| invalid())?;
        let (signin, pairing) = match health.features {
            Some(features) => (features.contains("signin"), features.contains("pairing")),
            None => (
                wire::signin_available(&self.get("auth", None, None).await?)
                    .map_err(|_| invalid())?,
                false,
            ),
        };
        Ok(json!({"signin":signin,"pairing":pairing}))
    }
    async fn get(
        &self,
        route: &str,
        token: Option<&str>,
        query: Option<(&str, &str)>,
    ) -> Result<Vec<u8>, Failure> {
        self.send(route, token, None, query).await
    }
    async fn post(
        &self,
        route: &str,
        token: Option<&str>,
        body: Value,
    ) -> Result<Vec<u8>, Failure> {
        self.send(route, token, Some(body), None).await
    }
    async fn send(
        &self,
        route: &str,
        token: Option<&str>,
        body: Option<Value>,
        query: Option<(&str, &str)>,
    ) -> Result<Vec<u8>, Failure> {
        if !matches!(
            route,
            "health" | "auth" | "auth/login" | "auth/poll" | "agents" | "pair" | "agent/projects"
        ) {
            return Err(invalid());
        }
        let url = self
            .base
            .join(&format!("api/v1/{route}"))
            .map_err(|_| invalid())?;
        let mut request = if let Some(body) = body {
            self.client.post(url).json(&body)
        } else {
            self.client.get(url)
        };
        request = request
            .header("accept", "application/json")
            .header("cache-control", "no-store");
        if let Some(token) = token {
            if !wire::valid_token(token) {
                return Err(invalid());
            }
            request = request.bearer_auth(token);
        }
        if let Some((key, value)) = query {
            request = request.query(&[(key, value)]);
        }
        let mut response = request.send().await.map_err(|_| Failure(StatusCode::BAD_GATEWAY, "Collaboration request failed; a token-producing request may have completed remotely. Reload before retrying"))?;
        if !response.status().is_success() {
            return Err(match response.status() {
                StatusCode::UNAUTHORIZED => Failure(StatusCode::UNAUTHORIZED, "Collaboration credential was rejected; repair is required"),
                StatusCode::FORBIDDEN | StatusCode::CONFLICT => Failure(StatusCode::CONFLICT, "Collaboration server refused this operation; review its configuration and rig identity"),
                StatusCode::TOO_MANY_REQUESTS => Failure(StatusCode::TOO_MANY_REQUESTS, "Collaboration server is rate limiting requests"),
                StatusCode::UNPROCESSABLE_ENTITY => Failure(StatusCode::UNPROCESSABLE_ENTITY, "Collaboration server refused the enrollment fields"),
                _ => Failure(StatusCode::BAD_GATEWAY, "Collaboration server returned an unsuccessful response; no automatic enrollment retry was made"),
            });
        }
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .split(';')
            .next()
            .unwrap_or("")
            .trim();
        if content_type != "application/json" || response.headers().contains_key("content-encoding")
        {
            return Err(invalid());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| invalid())? {
            if bytes.len().saturating_add(chunk.len()) > astrocollab::MAX_BODY_BYTES {
                return Err(invalid());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}
