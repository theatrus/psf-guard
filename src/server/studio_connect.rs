//! Connecting PSF Studio to this server: a short code approved in the
//! browser, or a one-time ticket from **Open in PSF-Studio**, traded for a
//! Studio token.
//!
//! 1. Studio starts a connect at `POST /api/auth/studio/connect` and gets a
//!    secret connect id and a short user code.
//! 2. The person opens the **Connect PSF Studio** page, signed in, checks
//!    the code and allows it at `POST /api/auth/studio/approve`.
//! 3. Studio asks `POST /api/auth/studio/token` with the connect id until
//!    the answer is approved, denied or expired. The approved answer
//!    carries the token, once.
//!
//! A ticket, which a signed-in person's click mints, stands for an approval:
//! Studio trades it at the same token endpoint.
//!
//! The token is minted when Studio collects it, so the server never holds a
//! token's secret. Pending connects and tickets live in memory: a restart
//! means starting the connect again.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::{Deserialize, Serialize};

use crate::auth_registry::{hash_token, unix_now, AuthRegistry, TokenScope};
use crate::server::api::ApiResponse;
use crate::server::auth::{AccessRole, RequestAccess};
use crate::server::handlers::{require_registry_path, AppError};
use crate::server::state::AppState;

/// How long a code waits for approval.
pub const CONNECT_SECONDS: i64 = 10 * 60;
/// How long a ticket from a link lasts.
pub const TICKET_SECONDS: i64 = 60;
/// How long a Studio token lasts.
pub const STUDIO_TOKEN_DAYS: u32 = 90;
/// How often Studio asks whether a code was approved.
pub const POLL_SECONDS: u64 = 2;
/// The most connects that wait at once; the oldest goes first.
const MAX_PENDING: usize = 64;
const MAX_CLIENT_NAME: usize = 60;
/// Letters and digits a person cannot mistake for one another: no 0, O, 1,
/// I or L.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const CODE_LENGTH: usize = 8;

#[derive(Clone, Debug, PartialEq)]
enum Decision {
    Waiting,
    /// Allowed by this user.
    Allowed(String),
    Denied,
}

#[derive(Clone, Debug)]
struct Pending {
    user_code: String,
    client_name: String,
    expires_at: i64,
    decision: Decision,
}

#[derive(Clone, Debug)]
struct Ticket {
    username: String,
    expires_at: i64,
}

/// Connects waiting for approval, and tickets waiting to be traded, by the
/// hash of their secret.
#[derive(Default)]
pub struct StudioConnects {
    pending: Mutex<HashMap<String, Pending>>,
    tickets: Mutex<HashMap<String, Ticket>>,
}

fn random_hex() -> String {
    let random: [u8; 32] = rand::random();
    random.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// A user code as a person reads it: two groups of four.
fn user_code() -> String {
    let random: [u8; CODE_LENGTH] = rand::random();
    let letters: String = random
        .iter()
        .map(|byte| CODE_ALPHABET[usize::from(*byte) % CODE_ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &letters[..4], &letters[4..])
}

/// A code as typed or pasted: capitals, without spaces, with the dash.
fn normal_code(code: &str) -> String {
    let letters: String = code
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|letter| letter.to_ascii_uppercase())
        .collect();
    if letters.len() == CODE_LENGTH {
        format!("{}-{}", &letters[..4], &letters[4..])
    } else {
        letters
    }
}

/// A computer's name as Studio sent it, kept short and printable.
fn client_name(name: Option<&str>) -> String {
    let name: String = name
        .unwrap_or_default()
        .chars()
        .filter(|letter| !letter.is_control())
        .take(MAX_CLIENT_NAME)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "a computer".to_string()
    } else {
        name.to_string()
    }
}

/// What trading a connect id or a ticket answers.
#[derive(Debug, PartialEq)]
enum Trade {
    Waiting,
    Denied,
    Expired,
    /// Mint a token for this user and computer.
    Mint {
        username: String,
        client_name: String,
    },
}

impl StudioConnects {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a connect: the connect id, which only Studio sees, and the user
    /// code, which the person checks.
    fn start(&self, client_name: String, now: i64) -> (String, String, i64) {
        let connect_id = random_hex();
        let code = user_code();
        let expires_at = now + CONNECT_SECONDS;
        let mut pending = self.pending.lock().unwrap();
        pending.retain(|_, entry| entry.expires_at > now);
        while pending.len() >= MAX_PENDING {
            let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, entry)| entry.expires_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            pending.remove(&oldest);
        }
        pending.insert(
            hash_token(&connect_id),
            Pending {
                user_code: code.clone(),
                client_name,
                expires_at,
                decision: Decision::Waiting,
            },
        );
        (connect_id, code, expires_at)
    }

    /// The computer a waiting code is for, and when the code expires.
    fn waiting(&self, code: &str, now: i64) -> Option<(String, i64)> {
        let code = normal_code(code);
        self.pending
            .lock()
            .unwrap()
            .values()
            .find(|entry| {
                entry.user_code == code
                    && entry.expires_at > now
                    && entry.decision == Decision::Waiting
            })
            .map(|entry| (entry.client_name.clone(), entry.expires_at))
    }

    /// Allow or deny a waiting code. `None` when no code waits by that name.
    fn decide(&self, code: &str, decision: Decision, now: i64) -> Option<String> {
        let code = normal_code(code);
        let mut pending = self.pending.lock().unwrap();
        let entry = pending.values_mut().find(|entry| {
            entry.user_code == code && entry.expires_at > now && entry.decision == Decision::Waiting
        })?;
        entry.decision = decision;
        Some(entry.client_name.clone())
    }

    /// Trade a connect id: what its code's approval came to. An approved or
    /// denied connect is answered once and then forgotten.
    fn trade_connect(&self, connect_id: &str, now: i64) -> Trade {
        let key = hash_token(connect_id);
        let mut pending = self.pending.lock().unwrap();
        let Some(entry) = pending.get(&key) else {
            return Trade::Expired;
        };
        if entry.expires_at <= now {
            pending.remove(&key);
            return Trade::Expired;
        }
        match entry.decision.clone() {
            Decision::Waiting => Trade::Waiting,
            Decision::Denied => {
                pending.remove(&key);
                Trade::Denied
            }
            Decision::Allowed(username) => {
                let entry = pending.remove(&key).expect("the entry just read");
                Trade::Mint {
                    username,
                    client_name: entry.client_name,
                }
            }
        }
    }

    /// Mint a ticket for a signed-in person's click.
    fn issue_ticket(&self, username: &str, now: i64) -> (String, i64) {
        let ticket = random_hex();
        let expires_at = now + TICKET_SECONDS;
        let mut tickets = self.tickets.lock().unwrap();
        tickets.retain(|_, entry| entry.expires_at > now);
        tickets.insert(
            hash_token(&ticket),
            Ticket {
                username: username.to_string(),
                expires_at,
            },
        );
        (ticket, expires_at)
    }

    /// Trade a ticket, once.
    fn trade_ticket(&self, ticket: &str, client_name: String, now: i64) -> Trade {
        match self.tickets.lock().unwrap().remove(&hash_token(ticket)) {
            Some(entry) if entry.expires_at > now => Trade::Mint {
                username: entry.username,
                client_name,
            },
            _ => Trade::Expired,
        }
    }
}

/// The signed-in browser user a request comes from, or why it is refused.
fn session_user(state: &AppState, access: &RequestAccess) -> Result<String, AppError> {
    if state.server_auth().is_none() {
        return Err(AppError::BadRequest(
            "This server has no sign-in, so PSF Studio connects without a code".to_string(),
        ));
    }
    if access.api_token {
        return Err(AppError::Forbidden(
            "Sign in with a browser to connect PSF Studio".to_string(),
        ));
    }
    access
        .username
        .clone()
        .ok_or_else(|| AppError::Forbidden("Sign in to connect PSF Studio".to_string()))
}

fn too_many() -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(axum::http::header::RETRY_AFTER, "1")],
        Json(ApiResponse::<()>::error(
            "Too many attempts; wait a moment and try again".to_string(),
        )),
    )
        .into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct ConnectRequest {
    #[serde(default)]
    pub client_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ConnectStarted {
    /// The secret Studio trades for the token. Only Studio sees it.
    pub connect_id: String,
    /// The code the person checks on the approval page.
    pub user_code: String,
    /// The approval page, from the server's root.
    pub approve_path: String,
    pub expires_at: i64,
    pub poll_seconds: u64,
}

/// `POST /api/auth/studio/connect`, open without sign-in.
pub async fn connect_route(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ConnectRequest>,
) -> Response {
    let Some(auth) = state.server_auth() else {
        return AppError::BadRequest(
            "This server has no sign-in, so PSF Studio connects without a code".to_string(),
        )
        .into_response();
    };
    if !auth.claim_login_slot() {
        return too_many();
    }
    let name = client_name(request.client_name.as_deref());
    let (connect_id, user_code, expires_at) = state.studio_connects.start(name.clone(), unix_now());
    tracing::info!("PSF Studio on {name} started a connect");
    Json(ApiResponse::success(ConnectStarted {
        approve_path: format!("/#/studio-connect?code={user_code}"),
        connect_id,
        user_code,
        expires_at,
        poll_seconds: POLL_SECONDS,
    }))
    .into_response()
}

#[derive(Debug, Deserialize)]
pub struct CodeQuery {
    pub code: String,
}

#[derive(Debug, Serialize)]
pub struct WaitingConnect {
    pub user_code: String,
    pub client_name: String,
    pub expires_at: i64,
    /// Whether a Studio connection of this account may save documents.
    pub saves_documents: bool,
}

/// `GET /api/auth/studio/approve?code=…`: the approval page's details for
/// a waiting code, for a signed-in person.
pub async fn waiting_route(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Query(query): Query<CodeQuery>,
) -> Response {
    if let Err(error) = session_user(&state, &access) {
        return error.into_response();
    }
    match state.studio_connects.waiting(&query.code, unix_now()) {
        Some((client_name, expires_at)) => Json(ApiResponse::success(WaitingConnect {
            user_code: normal_code(&query.code),
            client_name,
            expires_at,
            saves_documents: access.role == AccessRole::ReadWrite,
        }))
        .into_response(),
        None => AppError::NotFoundMessage(
            "No PSF Studio is waiting with this code. It may have expired: start the \
             connection again in PSF Studio."
                .to_string(),
        )
        .into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub struct ApproveRequest {
    pub code: String,
    pub allow: bool,
}

#[derive(Debug, Serialize)]
pub struct Approved {
    pub client_name: String,
    pub allowed: bool,
}

/// `POST /api/auth/studio/approve`: allow or deny a waiting code, for a
/// signed-in person.
pub async fn approve_route(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Json(request): Json<ApproveRequest>,
) -> Response {
    let username = match session_user(&state, &access) {
        Ok(username) => username,
        Err(error) => return error.into_response(),
    };
    let decision = if request.allow {
        Decision::Allowed(username.clone())
    } else {
        Decision::Denied
    };
    match state
        .studio_connects
        .decide(&request.code, decision, unix_now())
    {
        Some(client_name) => {
            tracing::info!(
                "{username} {} PSF Studio on {client_name}",
                if request.allow { "allowed" } else { "denied" }
            );
            Json(ApiResponse::success(Approved {
                client_name,
                allowed: request.allow,
            }))
            .into_response()
        }
        None => {
            // A wrong code spends a sign-in attempt, so codes cannot be
            // guessed quickly.
            if let Some(auth) = state.server_auth()
                && !auth.claim_login_slot()
            {
                return too_many();
            }
            AppError::NotFoundMessage(
                "No PSF Studio is waiting with this code. Check the code PSF Studio shows."
                    .to_string(),
            )
            .into_response()
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TicketIssued {
    pub ticket: String,
    pub expires_at: i64,
}

/// `POST /api/auth/studio/ticket`: a one-time ticket for the link a
/// signed-in person's **Open in PSF-Studio** click opens.
pub async fn ticket_route(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
) -> Response {
    let username = match session_user(&state, &access) {
        Ok(username) => username,
        Err(error) => return error.into_response(),
    };
    let (ticket, expires_at) = state.studio_connects.issue_ticket(&username, unix_now());
    Json(ApiResponse::success(TicketIssued { ticket, expires_at })).into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct TokenRequest {
    #[serde(default)]
    pub connect_id: Option<String>,
    #[serde(default)]
    pub ticket: Option<String>,
    /// For a ticket: the computer's name, for the token's label.
    #[serde(default)]
    pub client_name: Option<String>,
}

#[derive(Debug, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TokenStatus {
    Waiting,
    Denied,
    Expired,
    Approved,
}

#[derive(Debug, Serialize)]
pub struct TokenAnswer {
    pub status: TokenStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// The role of the account the token belongs to.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<AccessRole>,
    /// Whether the token may save Studio documents.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saves_documents: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

impl TokenAnswer {
    fn only(status: TokenStatus) -> Self {
        Self {
            status,
            token: None,
            username: None,
            role: None,
            saves_documents: None,
            expires_at: None,
        }
    }
}

/// `POST /api/auth/studio/token`, open without sign-in: trade a connect id
/// or a ticket.
pub async fn token_route(
    State(state): State<Arc<AppState>>,
    Json(request): Json<TokenRequest>,
) -> Response {
    let Some(auth) = state.server_auth() else {
        return AppError::BadRequest(
            "This server has no sign-in, so PSF Studio connects without a token".to_string(),
        )
        .into_response();
    };
    let now = unix_now();
    let trade = match (&request.connect_id, &request.ticket) {
        (Some(connect_id), None) => state.studio_connects.trade_connect(connect_id, now),
        (None, Some(ticket)) => {
            if !auth.claim_login_slot() {
                return too_many();
            }
            state.studio_connects.trade_ticket(
                ticket,
                client_name(request.client_name.as_deref()),
                now,
            )
        }
        _ => {
            return AppError::BadRequest("Send a connect_id or a ticket".to_string())
                .into_response();
        }
    };
    let (username, client_name) = match trade {
        Trade::Waiting => {
            return Json(ApiResponse::success(TokenAnswer::only(
                TokenStatus::Waiting,
            )))
            .into_response()
        }
        Trade::Denied => {
            return Json(ApiResponse::success(TokenAnswer::only(TokenStatus::Denied)))
                .into_response()
        }
        Trade::Expired => {
            return Json(ApiResponse::success(TokenAnswer::only(
                TokenStatus::Expired,
            )))
            .into_response()
        }
        Trade::Mint {
            username,
            client_name,
        } => (username, client_name),
    };
    let registry_path = match require_registry_path(&state) {
        Ok(path) => AuthRegistry::path_for_database_registry(&path),
        Err(error) => return error.into_response(),
    };
    let label = format!("PSF Studio on {client_name}");
    let worker = auth.clone();
    let minted = tokio::task::spawn_blocking(move || {
        worker.create_token(
            &registry_path,
            &username,
            &label,
            true,
            Some(STUDIO_TOKEN_DAYS),
            TokenScope::Studio,
        )
    })
    .await;
    let minted = match minted {
        Ok(Ok(minted)) => minted,
        Ok(Err(error)) => return AppError::BadRequest(error.to_string()).into_response(),
        Err(error) => {
            return AppError::InternalError(format!("Token task failed: {error}")).into_response();
        }
    };
    let user_role = auth
        .token_access(&minted.token)
        .map_or(AccessRole::ReadOnly, |grant| grant.user_role);
    tracing::info!(
        "{} connected PSF Studio on {client_name}",
        minted.summary.username
    );
    Json(ApiResponse::success(TokenAnswer {
        status: TokenStatus::Approved,
        username: Some(minted.summary.username.clone()),
        role: Some(user_role),
        saves_documents: Some(user_role == AccessRole::ReadWrite),
        expires_at: minted.summary.expires_at,
        token: Some(minted.token),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_code_is_two_groups_of_letters_a_person_cannot_misread() {
        let code = user_code();
        assert_eq!(code.len(), 9);
        assert_eq!(&code[4..5], "-");
        assert!(code
            .chars()
            .filter(|letter| *letter != '-')
            .all(|letter| CODE_ALPHABET.contains(&(letter as u8))));
        assert_eq!(normal_code(" kq7m 4pxz "), "KQ7M-4PXZ");
        assert_eq!(normal_code("KQ7M-4PXZ"), "KQ7M-4PXZ");
    }

    #[test]
    fn an_allowed_code_mints_once() {
        let connects = StudioConnects::new();
        let (id, code, _) = connects.start("Mac".to_string(), 100);
        assert_eq!(connects.trade_connect(&id, 101), Trade::Waiting);
        assert_eq!(
            connects.waiting(&code.to_lowercase(), 101),
            Some(("Mac".to_string(), 100 + CONNECT_SECONDS))
        );
        assert_eq!(
            connects.decide(&code, Decision::Allowed("ana".to_string()), 102),
            Some("Mac".to_string())
        );
        // Decided once: the code no longer waits.
        assert_eq!(connects.waiting(&code, 103), None);
        assert_eq!(
            connects.decide(&code, Decision::Denied, 103),
            None,
            "a decided code cannot be decided again"
        );
        assert_eq!(
            connects.trade_connect(&id, 104),
            Trade::Mint {
                username: "ana".to_string(),
                client_name: "Mac".to_string()
            }
        );
        assert_eq!(connects.trade_connect(&id, 105), Trade::Expired);
    }

    #[test]
    fn a_denied_or_late_code_mints_nothing() {
        let connects = StudioConnects::new();
        let (id, code, _) = connects.start("Mac".to_string(), 100);
        connects.decide(&code, Decision::Denied, 101);
        assert_eq!(connects.trade_connect(&id, 102), Trade::Denied);
        assert_eq!(connects.trade_connect(&id, 103), Trade::Expired);

        let (id, code, expires_at) = connects.start("Mac".to_string(), 100);
        assert_eq!(
            connects.decide(&code, Decision::Allowed("ana".to_string()), expires_at),
            None,
            "an expired code cannot be allowed"
        );
        assert_eq!(connects.trade_connect(&id, expires_at), Trade::Expired);
        assert_eq!(connects.trade_connect("not an id", 100), Trade::Expired);
    }

    #[test]
    fn a_ticket_trades_once_and_only_in_time() {
        let connects = StudioConnects::new();
        let (ticket, expires_at) = connects.issue_ticket("ana", 100);
        assert_eq!(
            connects.trade_ticket(&ticket, "Mac".to_string(), 101),
            Trade::Mint {
                username: "ana".to_string(),
                client_name: "Mac".to_string()
            }
        );
        assert_eq!(
            connects.trade_ticket(&ticket, "Mac".to_string(), 102),
            Trade::Expired
        );
        let (late, _) = connects.issue_ticket("ana", 100);
        assert_eq!(
            connects.trade_ticket(&late, "Mac".to_string(), expires_at),
            Trade::Expired
        );
    }

    #[test]
    fn waiting_connects_are_capped() {
        let connects = StudioConnects::new();
        for at in 0..(MAX_PENDING as i64 + 10) {
            connects.start("Mac".to_string(), 100 + at);
        }
        assert_eq!(connects.pending.lock().unwrap().len(), MAX_PENDING);
    }

    #[test]
    fn a_computer_name_is_short_and_printable() {
        assert_eq!(client_name(None), "a computer");
        assert_eq!(client_name(Some(" \u{7}Mac\n")), "Mac");
        assert_eq!(client_name(Some(&"x".repeat(200))).len(), MAX_CLIENT_NAME);
    }
}
