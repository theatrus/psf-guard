//! Axum extractors for the multi-DB server.
//!
//! `DbContext` reads `{db_id}` from the matched path, looks it up in
//! `AppState.databases`, and yields an `Arc<DatabaseContext>`. Handlers
//! attached to nested routes (`/api/db/{db_id}/...`) take this extractor
//! instead of `State<Arc<AppState>>` for per-DB work.

use std::ops::Deref;
use std::sync::Arc;

use axum::extract::{FromRef, FromRequestParts, RawPathParams};
use axum::http::request::Parts;
use axum::http::StatusCode;

use crate::server::database_context::DatabaseContext;
use crate::server::state::AppState;

/// Resolves the `{db_id}` path param to a live `DatabaseContext`. 404 if the
/// slug isn't configured; 400 if the route was wired without a `{db_id}`
/// segment.
pub struct DbContext(pub Arc<DatabaseContext>);

impl Deref for DbContext {
    type Target = DatabaseContext;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<S> FromRequestParts<S> for DbContext
where
    S: Send + Sync,
    Arc<AppState>: FromRef<S>,
{
    type Rejection = (StatusCode, String);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app_state = Arc::<AppState>::from_ref(state);
        let raw = RawPathParams::from_request_parts(parts, state)
            .await
            .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;
        let db_id = raw
            .iter()
            .find(|(k, _)| *k == "db_id")
            .map(|(_, v)| v.to_string())
            .ok_or((
                StatusCode::BAD_REQUEST,
                "route is missing required {db_id} segment".to_string(),
            ))?;
        app_state.get_database(&db_id).map(DbContext).ok_or((
            StatusCode::NOT_FOUND,
            format!("unknown database: {}", db_id),
        ))
    }
}
