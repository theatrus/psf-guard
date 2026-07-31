use axum::{
    extract::{Request, State},
    http::{
        header::{CACHE_CONTROL, COOKIE, SET_COOKIE},
        HeaderMap, HeaderValue, Method, StatusCode,
    },
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{
    config::{ServerAuthConfig, ServerAuthCredentialConfig},
    server::{api::ApiResponse, state::AppState},
};

const SESSION_COOKIE: &str = "psf_guard_session";
const DEFAULT_SESSION_HOURS: u64 = 24 * 7;
const MAX_SESSIONS_PER_USER: usize = 128;
const LOGIN_ATTEMPTS_PER_SECOND: f64 = 4.0;
const LOGIN_ATTEMPT_BURST: f64 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessRole {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Clone)]
pub struct RequestAccess {
    pub role: AccessRole,
}

#[derive(Clone)]
pub struct ServerAuth {
    users: Vec<AuthUser>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    login_rate_limit: Arc<Mutex<LoginRateLimit>>,
    session_ttl: Duration,
    secure_cookie: bool,
    allow_read_only_compute: bool,
}

#[derive(Clone)]
struct AuthUser {
    username: String,
    password_digest: [u8; 32],
    role: AccessRole,
}

#[derive(Clone)]
struct Session {
    username: String,
    role: AccessRole,
    expires_at: Instant,
}

struct LoginRateLimit {
    available: f64,
    last_refill: Instant,
}

impl std::fmt::Debug for ServerAuth {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ServerAuth")
            .field(
                "users",
                &self
                    .users
                    .iter()
                    .map(|user| (&user.username, user.role))
                    .collect::<Vec<_>>(),
            )
            .field("session_ttl", &self.session_ttl)
            .field("secure_cookie", &self.secure_cookie)
            .finish()
    }
}

impl ServerAuth {
    pub fn from_config(config: &ServerAuthConfig) -> anyhow::Result<Self> {
        let mut users = Vec::new();
        if let Some(credentials) = &config.read_only {
            users.push(AuthUser::from_config(credentials, AccessRole::ReadOnly)?);
        }
        if let Some(credentials) = &config.read_write {
            users.push(AuthUser::from_config(credentials, AccessRole::ReadWrite)?);
        }
        if users.is_empty() {
            anyhow::bail!("server.auth needs at least one of read_only or read_write");
        }
        if users.len() == 2 && users[0].username == users[1].username {
            anyhow::bail!("server.auth read_only and read_write usernames must differ");
        }
        let session_hours = config.session_hours.unwrap_or(DEFAULT_SESSION_HOURS);
        if !(1..=24 * 90).contains(&session_hours) {
            anyhow::bail!("server.auth.session_hours must be between 1 and 2160");
        }
        Ok(Self {
            users,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            login_rate_limit: Arc::new(Mutex::new(LoginRateLimit {
                available: LOGIN_ATTEMPT_BURST,
                last_refill: Instant::now(),
            })),
            session_ttl: Duration::from_secs(session_hours * 60 * 60),
            secure_cookie: config.secure_cookie,
            allow_read_only_compute: config.allow_read_only_compute,
        })
    }

    fn authenticate_password(&self, username: &str, password: &str) -> Option<&AuthUser> {
        let candidate = Sha256::digest(password.as_bytes());
        self.users.iter().find(|user| {
            let password_matches =
                constant_time_eq(user.password_digest.as_slice(), candidate.as_slice());
            user.username == username && password_matches
        })
    }

    fn claim_login_slot(&self) -> bool {
        let mut limit = self.login_rate_limit.lock().unwrap();
        let now = Instant::now();
        let replenished = limit.available
            + now.duration_since(limit.last_refill).as_secs_f64() * LOGIN_ATTEMPTS_PER_SECOND;
        limit.available = replenished.min(LOGIN_ATTEMPT_BURST);
        limit.last_refill = now;
        if limit.available < 1.0 {
            return false;
        }
        limit.available -= 1.0;
        true
    }

    fn create_session(&self, user: &AuthUser) -> String {
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let session = Session {
            username: user.username.clone(),
            role: user.role,
            expires_at: Instant::now() + self.session_ttl,
        };
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, session| session.expires_at > Instant::now());
        if sessions
            .values()
            .filter(|session| session.username == user.username)
            .count()
            >= MAX_SESSIONS_PER_USER
            && let Some(oldest) = sessions
                .iter()
                .filter(|(_, session)| session.username == user.username)
                .min_by_key(|(_, session)| session.expires_at)
                .map(|(token, _)| token.clone())
        {
            sessions.remove(&oldest);
        }
        sessions.insert(token.clone(), session);
        token
    }

    fn session(&self, token: &str) -> Option<Session> {
        let mut sessions = self.sessions.lock().unwrap();
        sessions.retain(|_, session| session.expires_at > Instant::now());
        sessions.get(token).cloned()
    }

    fn remove_session(&self, token: &str) {
        self.sessions.lock().unwrap().remove(token);
    }

    fn cookie(&self, token: &str) -> HeaderValue {
        let mut value = format!(
            "{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}",
            self.session_ttl.as_secs()
        );
        if self.secure_cookie {
            value.push_str("; Secure");
        }
        HeaderValue::from_str(&value).expect("session cookie contains safe characters")
    }

    fn clear_cookie(&self) -> HeaderValue {
        let mut value = format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0");
        if self.secure_cookie {
            value.push_str("; Secure");
        }
        HeaderValue::from_str(&value).expect("clear cookie contains safe characters")
    }

    fn can_compute(&self, role: AccessRole) -> bool {
        role == AccessRole::ReadWrite || self.allow_read_only_compute
    }
}

impl AuthUser {
    fn from_config(
        credentials: &ServerAuthCredentialConfig,
        role: AccessRole,
    ) -> anyhow::Result<Self> {
        let username = credentials.username.trim();
        if username.is_empty() || username.contains(':') {
            anyhow::bail!(
                "server.auth {} username must be non-empty and cannot contain ':'",
                role.config_name()
            );
        }
        let password = read_password(credentials, role)?;
        if password.is_empty() {
            anyhow::bail!(
                "server.auth {} password must not be empty",
                role.config_name()
            );
        }
        Ok(Self {
            username: username.to_string(),
            password_digest: Sha256::digest(password.as_bytes()).into(),
            role,
        })
    }
}

impl AccessRole {
    fn config_name(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::ReadWrite => "read_write",
        }
    }
}

fn read_password(
    credentials: &ServerAuthCredentialConfig,
    role: AccessRole,
) -> anyhow::Result<String> {
    match (&credentials.password, &credentials.password_file) {
        (Some(_), Some(_)) => anyhow::bail!(
            "server.auth {} sets both password and password_file; use one",
            role.config_name()
        ),
        (Some(password), None) => Ok(password.clone()),
        (None, Some(path)) => std::fs::read_to_string(path)
            .map(|password| password.trim().to_string())
            .map_err(|error| {
                anyhow::anyhow!(
                    "reading server.auth {} password from {}: {}",
                    role.config_name(),
                    path,
                    error
                )
            }),
        (None, None) => anyhow::bail!(
            "server.auth {} needs password or password_file",
            role.config_name()
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Serialize)]
pub struct AuthStatus {
    pub authentication_required: bool,
    pub authenticated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<AccessRole>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub can_compute: bool,
}

pub async fn status(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let auth_status = if let Some(auth) = state.server_auth() {
        let session = session_from_headers(&auth, &headers);
        AuthStatus {
            authentication_required: true,
            authenticated: session.is_some(),
            role: session.as_ref().map(|session| session.role),
            can_compute: session
                .as_ref()
                .is_some_and(|session| auth.can_compute(session.role)),
            username: session.map(|session| session.username),
        }
    } else {
        AuthStatus {
            authentication_required: false,
            authenticated: true,
            role: Some(AccessRole::ReadWrite),
            username: None,
            can_compute: true,
        }
    };
    (
        [(CACHE_CONTROL, "no-store")],
        Json(ApiResponse::success(auth_status)),
    )
        .into_response()
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(request): Json<LoginRequest>,
) -> Response {
    let Some(auth) = state.server_auth() else {
        return Json(ApiResponse::success(AuthStatus {
            authentication_required: false,
            authenticated: true,
            role: Some(AccessRole::ReadWrite),
            username: None,
            can_compute: true,
        }))
        .into_response();
    };
    if !auth.claim_login_slot() {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            [
                (CACHE_CONTROL, HeaderValue::from_static("no-store")),
                (
                    axum::http::header::RETRY_AFTER,
                    HeaderValue::from_static("1"),
                ),
            ],
            Json(ApiResponse::<AuthStatus>::error(
                "Too many sign-in attempts; wait a moment and try again".to_string(),
            )),
        )
            .into_response();
    }
    let Some(user) = auth.authenticate_password(request.username.trim(), &request.password) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(CACHE_CONTROL, "no-store")],
            Json(ApiResponse::<AuthStatus>::error(
                "The username or password is incorrect".to_string(),
            )),
        )
            .into_response();
    };
    let token = auth.create_session(user);
    (
        StatusCode::OK,
        [
            (SET_COOKIE, auth.cookie(&token)),
            (CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        Json(ApiResponse::success(AuthStatus {
            authentication_required: true,
            authenticated: true,
            role: Some(user.role),
            username: Some(user.username.clone()),
            can_compute: auth.can_compute(user.role),
        })),
    )
        .into_response()
}

pub async fn logout(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let Some(auth) = state.server_auth() else {
        return StatusCode::NO_CONTENT.into_response();
    };
    if let Some(token) = cookie_value(&headers, SESSION_COOKIE) {
        auth.remove_session(token);
    }
    (
        StatusCode::NO_CONTENT,
        [
            (SET_COOKIE, auth.clear_cookie()),
            (CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
    )
        .into_response()
}

pub async fn authorize_api(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(auth) = state.server_auth() else {
        request.extensions_mut().insert(RequestAccess {
            role: AccessRole::ReadWrite,
        });
        return next.run(request).await;
    };

    let path = api_path(request.uri().path());
    if request.method() == Method::OPTIONS
        || is_public_auth_path(path)
        || uses_remote_bearer_token(path)
    {
        return next.run(request).await;
    }

    let Some(session) = session_from_headers(&auth, request.headers()) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(CACHE_CONTROL, "no-store")],
            Json(ApiResponse::<()>::error("Sign in to continue".to_string())),
        )
            .into_response();
    };
    let can_compute = auth.can_compute(session.role);
    if session.role == AccessRole::ReadOnly && requires_write(request.method(), path, can_compute) {
        return (
            StatusCode::FORBIDDEN,
            [(CACHE_CONTROL, "no-store")],
            Json(ApiResponse::<()>::error(
                "This account has read-only access".to_string(),
            )),
        )
            .into_response();
    }

    request
        .extensions_mut()
        .insert(RequestAccess { role: session.role });
    let mut response = next.run(request).await;
    mark_response_private(&mut response);
    response
}

fn session_from_headers(auth: &ServerAuth, headers: &HeaderMap) -> Option<Session> {
    auth.session(cookie_value(headers, SESSION_COOKIE)?)
}

fn cookie_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .map(str::trim)
        .find_map(|cookie| cookie.strip_prefix(&format!("{name}=")))
}

fn uses_remote_bearer_token(path: &str) -> bool {
    path.starts_with("/sync/v1/") || (path.starts_with("/db/") && path.ends_with("/images/upload"))
}

fn is_public_auth_path(path: &str) -> bool {
    matches!(path, "/auth/status" | "/auth/login" | "/auth/logout")
}

fn requires_write(method: &Method, path: &str, can_compute: bool) -> bool {
    if matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS) {
        return false;
    }
    if method != Method::POST {
        return true;
    }

    if path.ends_with("/images/generation-status") {
        return false;
    }
    if !can_compute {
        return true;
    }

    // These POSTs calculate derived display data. They may populate caches,
    // but they do not change catalog grades, plans, files, or server config.
    !(path.ends_with("/astrometry")
        || path.ends_with("/satellites")
        || path.contains("/stack-previews")
        || path == "/astrometry/catalogs/validate")
}

fn api_path(path: &str) -> &str {
    path.strip_prefix("/api")
        .filter(|remaining| remaining.starts_with('/'))
        .unwrap_or(path)
}

fn mark_response_private(response: &mut Response) {
    let directives = response
        .headers()
        .get(CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|directive| {
            !directive.eq_ignore_ascii_case("private")
                && !directive.eq_ignore_ascii_case("public")
                && !directive.is_empty()
        })
        .collect::<Vec<_>>();
    let value = if directives.is_empty() {
        "private".to_string()
    } else {
        format!("private, {}", directives.join(", "))
    };
    response.headers_mut().insert(
        CACHE_CONTROL,
        HeaderValue::from_str(&value).expect("cache directives came from a valid header"),
    );
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ServerAuthConfig {
        ServerAuthConfig {
            read_only: Some(ServerAuthCredentialConfig {
                username: "viewer".into(),
                password: Some("view-secret".into()),
                password_file: None,
            }),
            read_write: Some(ServerAuthCredentialConfig {
                username: "editor".into(),
                password: Some("edit-secret".into()),
                password_file: None,
            }),
            session_hours: Some(12),
            secure_cookie: true,
            allow_read_only_compute: false,
        }
    }

    #[test]
    fn accepts_both_roles_and_creates_expiring_secure_cookies() {
        let auth = ServerAuth::from_config(&test_config()).unwrap();
        let viewer = auth.authenticate_password("viewer", "view-secret").unwrap();
        assert_eq!(viewer.role, AccessRole::ReadOnly);
        assert!(auth.authenticate_password("viewer", "wrong").is_none());

        let token = auth.create_session(viewer);
        let cookie = auth.cookie(&token).to_str().unwrap().to_string();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Max-Age=43200"));
        assert!(cookie.contains("Secure"));
        assert_eq!(auth.session(&token).unwrap().username, "viewer");
    }

    #[test]
    fn session_store_evicts_only_within_the_same_user() {
        let auth = ServerAuth::from_config(&test_config()).unwrap();
        let viewer = auth.authenticate_password("viewer", "view-secret").unwrap();
        let editor = auth.authenticate_password("editor", "edit-secret").unwrap();
        let editor_session = auth.create_session(editor);
        let oldest_viewer = auth.create_session(viewer);
        for _ in 1..MAX_SESSIONS_PER_USER {
            auth.create_session(viewer);
        }
        auth.create_session(viewer);

        assert!(auth.session(&oldest_viewer).is_none());
        assert!(auth.session(&editor_session).is_some());
        assert_eq!(
            auth.sessions.lock().unwrap().len(),
            MAX_SESSIONS_PER_USER + 1
        );
    }

    #[test]
    fn read_only_rules_allow_derived_views_but_block_catalog_changes() {
        assert!(!requires_write(
            &Method::POST,
            "/db/test/images/generation-status",
            false,
        ));
        assert!(requires_write(
            &Method::POST,
            "/db/test/images/12/astrometry",
            false,
        ));
        assert!(!requires_write(
            &Method::POST,
            "/db/test/images/12/astrometry",
            true,
        ));
        assert!(requires_write(
            &Method::PUT,
            "/db/test/images/12/grade",
            true,
        ));
        assert!(requires_write(
            &Method::POST,
            "/db/test/analysis/quality-scan",
            true,
        ));
        assert!(requires_write(&Method::POST, "/databases/create", true,));
    }

    #[test]
    fn login_rate_limit_rejects_instead_of_queueing() {
        let auth = ServerAuth::from_config(&test_config()).unwrap();
        for _ in 0..LOGIN_ATTEMPT_BURST as usize {
            assert!(auth.claim_login_slot());
        }
        assert!(!auth.claim_login_slot());
    }

    #[test]
    fn only_current_session_endpoints_are_public() {
        assert!(is_public_auth_path("/auth/status"));
        assert!(is_public_auth_path("/auth/login"));
        assert!(is_public_auth_path("/auth/logout"));
        assert!(!is_public_auth_path("/auth/passkeys/enroll"));
    }

    #[test]
    fn remote_bearer_routes_stay_separate() {
        assert_eq!(api_path("/api/auth/status"), "/auth/status");
        assert_eq!(api_path("/auth/status"), "/auth/status");
        assert!(uses_remote_bearer_token("/sync/v1/capabilities"));
        assert!(uses_remote_bearer_token("/db/test/images/upload"));
        assert!(!uses_remote_bearer_token("/db/test/images/12/grade"));
    }

    #[test]
    fn authenticated_responses_cannot_enter_shared_caches() {
        let mut response = Response::builder()
            .header(CACHE_CONTROL, "public, max-age=86400")
            .body(axum::body::Body::empty())
            .unwrap();
        mark_response_private(&mut response);

        assert_eq!(response.headers()[CACHE_CONTROL], "private, max-age=86400");
    }

    #[test]
    fn config_rejects_empty_duplicate_or_ambiguous_credentials() {
        let mut duplicate = test_config();
        duplicate.read_write.as_mut().unwrap().username = "viewer".into();
        assert!(ServerAuth::from_config(&duplicate).is_err());

        let mut ambiguous = test_config();
        ambiguous.read_write.as_mut().unwrap().password_file = Some("secret".into());
        assert!(ServerAuth::from_config(&ambiguous).is_err());

        let empty = ServerAuthConfig {
            read_only: None,
            read_write: None,
            session_hours: None,
            secure_cookie: false,
            allow_read_only_compute: false,
        };
        assert!(ServerAuth::from_config(&empty).is_err());
    }
}
