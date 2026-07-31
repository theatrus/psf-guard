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
use std::{
    collections::{HashMap, HashSet},
    path::Path as FilePath,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use crate::{
    auth_registry::{AuthRegistry, AuthUserRecord},
    config::ServerAuthConfig,
    server::{api::ApiResponse, state::AppState},
};

pub use crate::auth_registry::AccessRole;

const SESSION_COOKIE: &str = "psf_guard_session";
const DEFAULT_SESSION_HOURS: u64 = 24 * 7;
const MAX_SESSIONS_PER_USER: usize = 128;
const LOGIN_ATTEMPTS_PER_SECOND: f64 = 4.0;
const LOGIN_ATTEMPT_BURST: f64 = 4.0;

#[derive(Debug, Clone)]
pub struct RequestAccess {
    pub role: AccessRole,
    pub username: Option<String>,
}

#[derive(Clone)]
pub struct ServerAuth {
    users: Arc<RwLock<Vec<AuthUser>>>,
    sessions: Arc<Mutex<HashMap<String, Session>>>,
    login_rate_limit: Arc<Mutex<LoginRateLimit>>,
    user_management_lock: Arc<Mutex<()>>,
    session_ttl: Duration,
    secure_cookie: bool,
    allow_read_only_compute: bool,
}

#[derive(Clone)]
struct AuthUser {
    username: String,
    password_hash: String,
    role: AccessRole,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthUserSummary {
    pub username: String,
    pub role: AccessRole,
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
                    .read()
                    .unwrap()
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
    pub fn from_sources(
        config: Option<&ServerAuthConfig>,
        registry: &AuthRegistry,
    ) -> anyhow::Result<Option<Self>> {
        let users = registry
            .users
            .iter()
            .map(|user| AuthUser {
                username: user.username.clone(),
                password_hash: user.password_hash().to_string(),
                role: user.role,
            })
            .collect::<Vec<_>>();
        if users.is_empty() {
            return Ok(None);
        }
        let session_hours = config
            .and_then(|config| config.session_hours)
            .unwrap_or(DEFAULT_SESSION_HOURS);
        if !(1..=24 * 90).contains(&session_hours) {
            anyhow::bail!("server.auth.session_hours must be between 1 and 2160");
        }
        Ok(Some(Self {
            users: Arc::new(RwLock::new(users)),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            login_rate_limit: Arc::new(Mutex::new(LoginRateLimit {
                available: LOGIN_ATTEMPT_BURST,
                last_refill: Instant::now(),
            })),
            user_management_lock: Arc::new(Mutex::new(())),
            session_ttl: Duration::from_secs(session_hours * 60 * 60),
            secure_cookie: config.is_none_or(|config| config.secure_cookie),
            allow_read_only_compute: config.is_some_and(|config| config.allow_read_only_compute),
        }))
    }

    async fn authenticate_password(&self, username: &str, password: &str) -> Option<AuthUser> {
        let (user, password_hash) = {
            let users = self.users.read().unwrap();
            let user = users.iter().find(|user| user.username == username).cloned();
            // Check a real hash even for an unknown name. This keeps a caller
            // from learning valid usernames from the Argon2 timing difference.
            let password_hash = user
                .as_ref()
                .unwrap_or_else(|| &users[0])
                .password_hash
                .clone();
            (user, password_hash)
        };
        let password = password.to_string();
        let matches = tokio::task::spawn_blocking(move || {
            crate::auth_registry::verify_password_hash(&password_hash, &password)
        })
        .await
        .unwrap_or(false);
        if matches {
            user
        } else {
            None
        }
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

    fn create_session(&self, user: &AuthUser) -> Option<String> {
        let users = self.users.read().unwrap();
        if !users.iter().any(|current| {
            current.username == user.username
                && current.password_hash == user.password_hash
                && current.role == user.role
        }) {
            return None;
        }
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
        Some(token)
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

    pub(crate) fn user_summaries(&self) -> Vec<AuthUserSummary> {
        let mut users = self
            .users
            .read()
            .unwrap()
            .iter()
            .map(|user| AuthUserSummary {
                username: user.username.clone(),
                role: user.role,
            })
            .collect::<Vec<_>>();
        users.sort_by(|left, right| left.username.cmp(&right.username));
        users
    }

    fn update_users<T>(
        &self,
        registry_path: &FilePath,
        update: impl FnOnce(&mut AuthRegistry) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let _guard = self.user_management_lock.lock().unwrap();
        let mut registry = AuthRegistry::load(registry_path)?;
        let result = update(&mut registry)?;
        let mut users = self.users.write().unwrap();
        let old_users = users.clone();
        let next_users = registry
            .users
            .iter()
            .map(|user| AuthUser {
                username: user.username.clone(),
                password_hash: user.password_hash().to_string(),
                role: user.role,
            })
            .collect::<Vec<_>>();
        if next_users.is_empty() {
            anyhow::bail!("cannot remove the final browser user while the server is running");
        }
        if !next_users
            .iter()
            .any(|user| user.role == AccessRole::ReadWrite)
        {
            anyhow::bail!("user management must keep at least one editor account");
        }
        registry.save(registry_path)?;

        let changed = old_users
            .iter()
            .filter(|old| {
                next_users
                    .iter()
                    .find(|next| next.username == old.username)
                    .is_none_or(|next| {
                        next.role != old.role || next.password_hash != old.password_hash
                    })
            })
            .map(|user| user.username.as_str())
            .collect::<HashSet<_>>();
        if !changed.is_empty() {
            self.sessions
                .lock()
                .unwrap()
                .retain(|_, session| !changed.contains(session.username.as_str()));
        }
        *users = next_users;
        Ok(result)
    }

    pub(crate) fn add_user(
        &self,
        registry_path: &FilePath,
        username: &str,
        role: AccessRole,
        password: &str,
    ) -> anyhow::Result<()> {
        let user = AuthUserRecord::new(username, role, password)?;
        self.update_users(registry_path, |registry| registry.add(user, false))
    }

    pub(crate) fn update_user(
        &self,
        registry_path: &FilePath,
        username: &str,
        role: AccessRole,
        password: Option<&str>,
    ) -> anyhow::Result<()> {
        self.update_users(registry_path, |registry| {
            let user = registry
                .find_mut(username)
                .ok_or_else(|| anyhow::anyhow!("user '{username}' does not exist"))?;
            user.role = role;
            if let Some(password) = password {
                user.set_password(password)?;
            }
            Ok(())
        })
    }

    pub(crate) fn remove_user(
        &self,
        registry_path: &FilePath,
        username: &str,
    ) -> anyhow::Result<()> {
        self.update_users(registry_path, |registry| registry.remove(username))
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
    let Some(user) = auth
        .authenticate_password(request.username.trim(), &request.password)
        .await
    else {
        return (
            StatusCode::UNAUTHORIZED,
            [(CACHE_CONTROL, "no-store")],
            Json(ApiResponse::<AuthStatus>::error(
                "The username or password is incorrect".to_string(),
            )),
        )
            .into_response();
    };
    let Some(token) = auth.create_session(&user) else {
        return (
            StatusCode::UNAUTHORIZED,
            [(CACHE_CONTROL, "no-store")],
            Json(ApiResponse::<AuthStatus>::error(
                "The account changed during sign-in; try again".to_string(),
            )),
        )
            .into_response();
    };
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
            username: None,
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

    request.extensions_mut().insert(RequestAccess {
        role: session.role,
        username: Some(session.username),
    });
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> ServerAuthConfig {
        ServerAuthConfig {
            session_hours: Some(12),
            secure_cookie: true,
            allow_read_only_compute: false,
        }
    }

    fn test_auth() -> ServerAuth {
        let mut registry = AuthRegistry::default();
        registry
            .add(
                AuthUserRecord::new("viewer", AccessRole::ReadOnly, "viewer-secret").unwrap(),
                false,
            )
            .unwrap();
        registry
            .add(
                AuthUserRecord::new("editor", AccessRole::ReadWrite, "editor-secret").unwrap(),
                false,
            )
            .unwrap();
        ServerAuth::from_sources(Some(&test_config()), &registry)
            .unwrap()
            .unwrap()
    }

    #[tokio::test]
    async fn accepts_both_roles_and_creates_expiring_secure_cookies() {
        let auth = test_auth();
        let viewer = auth
            .authenticate_password("viewer", "viewer-secret")
            .await
            .unwrap();
        assert_eq!(viewer.role, AccessRole::ReadOnly);
        assert!(auth
            .authenticate_password("viewer", "wrong")
            .await
            .is_none());

        let token = auth.create_session(&viewer).unwrap();
        let cookie = auth.cookie(&token).to_str().unwrap().to_string();
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("SameSite=Strict"));
        assert!(cookie.contains("Max-Age=43200"));
        assert!(cookie.contains("Secure"));
        assert_eq!(auth.session(&token).unwrap().username, "viewer");
    }

    #[tokio::test]
    async fn session_store_evicts_only_within_the_same_user() {
        let auth = test_auth();
        let viewer = auth
            .authenticate_password("viewer", "viewer-secret")
            .await
            .unwrap();
        let editor = auth
            .authenticate_password("editor", "editor-secret")
            .await
            .unwrap();
        let editor_session = auth.create_session(&editor).unwrap();
        let oldest_viewer = auth.create_session(&viewer).unwrap();
        for _ in 1..MAX_SESSIONS_PER_USER {
            let _ = auth.create_session(&viewer);
        }
        let _ = auth.create_session(&viewer);

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
        let auth = test_auth();
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
    fn session_policy_without_users_keeps_authentication_off() {
        assert!(
            ServerAuth::from_sources(Some(&test_config()), &AuthRegistry::default())
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn registry_users_enable_secure_auth_without_session_policy() {
        let mut registry = AuthRegistry::default();
        registry
            .add(
                crate::auth_registry::AuthUserRecord::new(
                    "viewer",
                    AccessRole::ReadOnly,
                    "viewer-password",
                )
                .unwrap(),
                false,
            )
            .unwrap();

        let auth = ServerAuth::from_sources(None, &registry).unwrap().unwrap();
        assert!(auth.secure_cookie);
        let user = auth
            .authenticate_password("viewer", "viewer-password")
            .await
            .unwrap();
        assert_eq!(user.role, AccessRole::ReadOnly);
    }

    #[tokio::test]
    async fn live_management_revokes_stale_sign_ins_and_keeps_an_editor() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("auth.json");
        let mut registry = AuthRegistry::default();
        registry
            .add(
                AuthUserRecord::new("only-editor", AccessRole::ReadWrite, "editor-password")
                    .unwrap(),
                false,
            )
            .unwrap();
        registry.save(&path).unwrap();
        let auth = ServerAuth::from_sources(None, &registry).unwrap().unwrap();
        let stale_user = auth
            .authenticate_password("only-editor", "editor-password")
            .await
            .unwrap();

        auth.update_user(
            &path,
            "only-editor",
            AccessRole::ReadWrite,
            Some("replacement-editor-password"),
        )
        .unwrap();
        assert!(auth.create_session(&stale_user).is_none());

        assert!(auth
            .update_user(&path, "only-editor", AccessRole::ReadOnly, None,)
            .is_err());
        assert!(auth.remove_user(&path, "only-editor").is_err());
        assert_eq!(
            AuthRegistry::load(&path).unwrap().users[0].role,
            AccessRole::ReadWrite
        );
    }
}
