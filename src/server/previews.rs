//! Previews a person drives in real time: a sync, an import or an export
//! shown before it runs. A newer preview of the same thing stops the one
//! before it ([`LatestRequests`]). The jobs route lists the previews under
//! way, and Stop ends one.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::server::api::ApiResponse;
use crate::server::job_line::latest::{LatestRequests, RunningPreview};
use crate::server::state::AppState;

/// What a preview previews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    /// A sync between two databases.
    Sync,
}

/// The previews under way, by kind. Each is keyed by the database whose
/// page asked for it.
#[derive(Default)]
pub struct Previews {
    /// Database syncs, by the local database.
    pub sync: LatestRequests<String>,
}

/// A preview under way, for the jobs route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRun {
    pub kind: PreviewKind,
    /// The database whose page asked for it.
    pub db_id: String,
    pub label: String,
    pub started_at: i64,
}

impl Previews {
    fn of(&self, kind: PreviewKind) -> &LatestRequests<String> {
        match kind {
            PreviewKind::Sync => &self.sync,
        }
    }

    /// Every preview under way, oldest first within each kind.
    pub fn running(&self) -> Vec<PreviewRun> {
        [PreviewKind::Sync]
            .into_iter()
            .flat_map(|kind| {
                self.of(kind).running().into_iter().map(
                    move |RunningPreview {
                              key,
                              label,
                              started_at,
                          }| PreviewRun {
                        kind,
                        db_id: key,
                        label,
                        started_at,
                    },
                )
            })
            .collect()
    }

    /// Stop the preview of `kind` for `db_id`. False when none runs.
    pub fn stop(&self, kind: PreviewKind, db_id: &str) -> bool {
        self.of(kind).stop(&db_id.to_string())
    }
}

/// `POST /api/previews/{kind}/{db_id}/stop`: stop a preview under way. The
/// request that started it then fails with a conflict. Replies whether a
/// preview was running.
pub async fn stop_route(
    State(state): State<Arc<AppState>>,
    Path((kind, db_id)): Path<(PreviewKind, String)>,
) -> Json<ApiResponse<bool>> {
    Json(ApiResponse::success(state.previews.stop(kind, &db_id)))
}
