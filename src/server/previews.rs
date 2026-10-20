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
    /// An import of frames into a database.
    Import,
    /// An AstroBin export of a database's frames.
    Export,
    /// A sync with another PSF Guard server.
    PeerSync,
    /// A sync another server pushed here, which waits for its Apply.
    Incoming,
}

const KINDS: [PreviewKind; 5] = [
    PreviewKind::Sync,
    PreviewKind::Import,
    PreviewKind::Export,
    PreviewKind::PeerSync,
    PreviewKind::Incoming,
];

/// The previews under way, by kind. Each is keyed by the database whose
/// page asked for it, except an incoming sync: see [`incoming_key`].
#[derive(Default)]
pub struct Previews {
    /// Database syncs, by the local database.
    pub sync: LatestRequests<String>,
    /// Imports, by the database they import into.
    pub import: LatestRequests<String>,
    /// AstroBin exports, by the database they export from.
    pub export: LatestRequests<String>,
    /// Syncs with another server, by the local database.
    pub peer_sync: LatestRequests<String>,
    /// Syncs pushed here, by the catalog and the catalog that sent them.
    pub incoming: LatestRequests<String>,
}

/// The key of an incoming sync: a newer push from the same source replaces
/// the older preview, and pushes from other sources run beside it. No
/// catalog id contains a colon.
pub fn incoming_key(catalog_id: &str, source_id: &str) -> String {
    format!("{catalog_id}:{source_id}")
}

/// A preview under way, for the jobs route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewRun {
    pub kind: PreviewKind,
    /// What Stop names.
    pub key: String,
    /// The database the preview works on.
    pub db_id: String,
    pub label: String,
    pub started_at: i64,
}

impl Previews {
    fn of(&self, kind: PreviewKind) -> &LatestRequests<String> {
        match kind {
            PreviewKind::Sync => &self.sync,
            PreviewKind::Import => &self.import,
            PreviewKind::Export => &self.export,
            PreviewKind::PeerSync => &self.peer_sync,
            PreviewKind::Incoming => &self.incoming,
        }
    }

    /// Every preview under way, oldest first within each kind.
    pub fn running(&self) -> Vec<PreviewRun> {
        KINDS
            .into_iter()
            .flat_map(|kind| {
                self.of(kind).running().into_iter().map(
                    move |RunningPreview {
                              key,
                              label,
                              started_at,
                          }| {
                        let db_id = match kind {
                            PreviewKind::Incoming => key
                                .split_once(':')
                                .map_or(key.as_str(), |(catalog, _)| catalog)
                                .to_string(),
                            _ => key.clone(),
                        };
                        PreviewRun {
                            kind,
                            key,
                            db_id,
                            label,
                            started_at,
                        }
                    },
                )
            })
            .collect()
    }

    /// Stop the preview of `kind` under `key`. False when none runs.
    pub fn stop(&self, kind: PreviewKind, key: &str) -> bool {
        self.of(kind).stop(&key.to_string())
    }
}

/// `POST /api/previews/{kind}/{key}/stop`: stop a preview under way. The
/// request that started it then fails with a conflict. Replies whether a
/// preview was running.
pub async fn stop_route(
    State(state): State<Arc<AppState>>,
    Path((kind, key)): Path<(PreviewKind, String)>,
) -> Json<ApiResponse<bool>> {
    Json(ApiResponse::success(state.previews.stop(kind, &key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_incoming_preview_works_on_its_catalog_and_stops_by_its_key() {
        let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        let previews = Previews::default();
        let key = incoming_key("home", "rig");
        let run = previews
            .incoming
            .start(&state, key.clone(), "Previewing a sync from rig");
        let other = previews.incoming.start(
            &state,
            incoming_key("home", "away"),
            "Previewing a sync from away",
        );
        let running = previews.running();
        assert_eq!(running.len(), 2, "two sources preview side by side");
        assert_eq!(
            (running[0].key.as_str(), running[0].db_id.as_str()),
            ("home:rig", "home")
        );
        assert!(previews.stop(PreviewKind::Incoming, &key));
        assert!(run.stop().requested() && !other.stop().requested());
    }
}
