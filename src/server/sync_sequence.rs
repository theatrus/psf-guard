//! What N.I.N.A.'s Advanced Sequencer is doing, as the PSF Guard Sync plugin
//! reports it, for the Live page.
//!
//! The plugin reads the running instructions once a second and sends a
//! report when they change, or every 15 s while a sequence runs. Reports are
//! kept in memory only: each describes the present and goes stale within
//! minutes, so a restart loses nothing worth keeping.

use std::{collections::HashMap, sync::Arc, sync::Mutex};

use axum::{extract::State, http::HeaderMap, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::server::{
    api::ApiResponse, handlers::AppError, remote_audit::AuditAction,
    remote_sync::authenticated_catalog, state::AppState,
};

/// Listed by `GET /api/sync/v1/capabilities` when the server takes reports.
pub const SEQUENCE_STATUS_CAPABILITY: &str = "sequence_status_v1";

/// How long a report stays current without another, unless it says.
pub const DEFAULT_FRESH_MS: u64 = 45_000;
const MIN_FRESH_MS: u64 = 15_000;
const MAX_FRESH_MS: u64 = 600_000;
/// Longest text kept from any field.
const MAX_TEXT_CHARS: usize = 200;

/// The running instruction the plugin judged most specific.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SequenceActivity {
    /// `exposing`, `autofocus`, `time_wait`, … Kinds this server does not
    /// know are kept; the page shows them as another instruction.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_seconds: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_temperature_c: Option<f64>,
}

/// When the plugin last saw each N.I.N.A. event this session.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RecentEvents {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_saved_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dither_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autofocus_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meridian_flip_ms: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct SequenceReport {
    session_id: Uuid,
    reported_at_ms: i64,
    running: bool,
    #[serde(default)]
    fresh_for_ms: Option<u64>,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    activity: Option<SequenceActivity>,
    #[serde(default)]
    recent: Option<RecentEvents>,
}

/// The newest report for one database.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SequenceStatus {
    pub session_id: Uuid,
    pub reported_at_ms: i64,
    pub received_at_ms: u64,
    pub fresh_for_ms: u64,
    pub running: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity: Option<SequenceActivity>,
    pub recent: RecentEvents,
}

impl SequenceStatus {
    /// Past its freshness lease: history, not the present.
    pub fn is_stale(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.received_at_ms) > self.fresh_for_ms
    }
}

fn clean(text: Option<String>) -> Option<String> {
    let cleaned: String = text?
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TEXT_CHARS)
        .collect();
    let cleaned = cleaned.trim();
    (!cleaned.is_empty()).then(|| cleaned.to_owned())
}

fn finite(value: Option<f64>) -> Option<f64> {
    value.filter(|value| value.is_finite())
}

impl SequenceReport {
    pub(crate) fn into_status(self, received_at_ms: u64) -> Result<SequenceStatus, AppError> {
        if self.session_id.is_nil() {
            return Err(AppError::BadRequest("session_id must not be nil".into()));
        }
        let activity = self.activity.map(|activity| SequenceActivity {
            kind: clean(Some(activity.kind)).unwrap_or_else(|| "other".into()),
            name: clean(activity.name),
            started_ms: activity.started_ms,
            until_ms: activity.until_ms,
            target_name: clean(activity.target_name),
            filter: clean(activity.filter),
            exposure_seconds: finite(activity.exposure_seconds),
            completed: activity.completed,
            total: activity.total,
            target_temperature_c: finite(activity.target_temperature_c),
        });
        Ok(SequenceStatus {
            session_id: self.session_id,
            reported_at_ms: self.reported_at_ms,
            received_at_ms,
            fresh_for_ms: self
                .fresh_for_ms
                .unwrap_or(DEFAULT_FRESH_MS)
                .clamp(MIN_FRESH_MS, MAX_FRESH_MS),
            running: self.running,
            operation: clean(self.operation),
            activity,
            recent: self.recent.unwrap_or_default(),
        })
    }
}

/// The newest sequence report per database ID.
#[derive(Default)]
pub struct SyncSequenceStatuses {
    latest: Mutex<HashMap<String, SequenceStatus>>,
}

impl SyncSequenceStatuses {
    /// Keep `status` unless it was observed before the stored report, so a
    /// late retry from a finished run never replaces the current one. Once
    /// the stored report is stale, any report replaces it, so a plugin whose
    /// clock moved back is not locked out.
    pub fn record(&self, catalog_id: &str, status: SequenceStatus) -> bool {
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if latest.get(catalog_id).is_some_and(|stored| {
            stored.reported_at_ms > status.reported_at_ms && !stored.is_stale(status.received_at_ms)
        }) {
            return false;
        }
        latest.insert(catalog_id.to_owned(), status);
        true
    }

    pub fn get(&self, catalog_id: &str) -> Option<SequenceStatus> {
        self.latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(catalog_id)
            .cloned()
    }
}

/// What the Live page shows about the Sync plugin on one rig.
#[derive(Debug, Clone, Serialize)]
pub struct SyncLive {
    /// The newest authenticated Sync call from any paired client.
    pub last_contact_ms: Option<u64>,
    /// The version that client reported.
    pub plugin_version: Option<String>,
    /// The newest action that was more than contact.
    pub latest_action: Option<crate::server::remote_audit::LatestAction>,
    /// Kinds of action whose newest attempt was refused or failed, newest
    /// first, so a later upload never hides a refused merge.
    pub problems: Vec<crate::server::remote_audit::LatestAction>,
    /// The newest sequence report; stale ones are history.
    pub sequence: Option<SequenceStatus>,
    pub sequence_stale: bool,
}

impl SyncLive {
    /// `None` for a database the Sync plugin has never reached and that
    /// takes nothing from it.
    pub fn for_catalog(
        state: &AppState,
        catalog: &crate::server::database_context::DatabaseContext,
        now_ms: u64,
    ) -> Option<Self> {
        let newest = state
            .sync_contacts
            .for_catalog(&catalog.id)
            .into_values()
            .max_by_key(|contact| contact.at);
        let latest_action = state.remote_audit.latest(&catalog.id);
        let problems = state.remote_audit.problems(&catalog.id);
        let sequence = state.sync_sequences.get(&catalog.id);
        let accepts = catalog
            .remote_image_upload
            .as_ref()
            .is_some_and(|config| config.sync_enabled || config.enabled);
        if !accepts && newest.is_none() && latest_action.is_none() && sequence.is_none() {
            return None;
        }
        Some(Self {
            last_contact_ms: newest
                .as_ref()
                .map(|contact| (contact.at.max(0) as u64).saturating_mul(1000)),
            plugin_version: newest.and_then(|contact| contact.version),
            latest_action,
            problems,
            sequence_stale: sequence
                .as_ref()
                .is_some_and(|status| status.is_stale(now_ms)),
            sequence,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct SequenceAnswer {
    accepted: bool,
    received_at_ms: u64,
}

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

/// `POST /api/sync/v1/sequence-status`.
pub async fn report(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(report): Json<SequenceReport>,
) -> Result<Json<ApiResponse<SequenceAnswer>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers, AuditAction::SequenceStatus)?;
    let received_at_ms = now_ms();
    let status = report.into_status(received_at_ms)?;
    let accepted = state.sync_sequences.record(&catalog.id, status);
    Ok(Json(ApiResponse::success(SequenceAnswer {
        accepted,
        received_at_ms,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(session: Uuid, reported_at_ms: i64) -> SequenceReport {
        serde_json::from_value(serde_json::json!({
            "session_id": session,
            "reported_at_ms": reported_at_ms,
            "running": true,
            "fresh_for_ms": 5,
            "operation": "M 31 \u{0007}› Imaging › Smart Exposure",
            "activity": {
                "kind": "exposing", "name": "  ", "target_name": "M 31", "filter": "L",
                "exposure_seconds": 300.0, "completed": 12, "total": 30, "future_field": true
            },
            "recent": { "dither_ms": 10 },
            "something_new": 1
        }))
        .unwrap()
    }

    #[test]
    fn a_report_is_cleaned_and_bounded() {
        let session = Uuid::new_v4();
        let status = report(session, 100).into_status(1_000).unwrap();
        assert_eq!(
            status.operation.as_deref(),
            Some("M 31 › Imaging › Smart Exposure")
        );
        assert_eq!(status.fresh_for_ms, MIN_FRESH_MS);
        let activity = status.activity.clone().unwrap();
        assert_eq!(activity.kind, "exposing");
        assert_eq!(activity.name, None);
        assert_eq!((activity.completed, activity.total), (Some(12), Some(30)));
        assert_eq!(status.recent.dither_ms, Some(10));
        assert!(!SequenceStatus {
            received_at_ms: 1_000,
            ..status.clone()
        }
        .is_stale(16_000));
        assert!(SequenceStatus {
            received_at_ms: 1_000,
            ..status
        }
        .is_stale(16_001));

        let long = "x".repeat(500);
        assert_eq!(clean(Some(long)).unwrap().chars().count(), MAX_TEXT_CHARS);
        assert!(report(Uuid::nil(), 1).into_status(1).is_err());
    }

    const TOKEN: &str = "observatory-key-long-enough-to-pass";

    fn served() -> (tempfile::TempDir, Arc<AppState>, axum::Router) {
        use crate::db_registry::{DbEntry, RemoteImageUploadConfig};
        use axum::routing::{get, post};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rig.sqlite");
        crate::ts_schema::create_fresh_db(&path).unwrap();
        let mut access = RemoteImageUploadConfig::default();
        access.set_token(TOKEN).unwrap();
        access.sync_enabled = true;
        let entry = DbEntry {
            ts_sync: None,
            id: "observatory".into(),
            name: "Observatory".into(),
            db_path: path.to_string_lossy().into_owned(),
            image_dirs: vec![dir.path().to_string_lossy().into_owned()],
            reject_archive: None,
            remote_image_upload: Some(access),
            export_dir: None,
            process_dir: None,
            autoimport: None,
            scan_rules: None,
            analyze_new_frames: false,
            pair_calibrated_copies: true,
            scan_calibrated_copies: false,
            processing_overrides: None,
        };
        let state = Arc::new(
            AppState::from_databases(
                vec![entry],
                dir.path().join("cache").to_string_lossy().into_owned(),
                crate::cli::PregenerationConfig::default(),
            )
            .unwrap(),
        );
        let router = axum::Router::new()
            .route(
                "/api/sync/v1/capabilities",
                get(crate::server::remote_sync::capabilities),
            )
            .route("/api/sync/v1/sequence-status", post(super::report))
            .with_state(state.clone());
        (dir, state, router)
    }

    async fn send(
        router: &axum::Router,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: serde_json::Value,
    ) -> (u16, serde_json::Value) {
        use tower::ServiceExt;
        let mut request = axum::http::Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .header("x-psf-guard-plugin-version", "1.5.0.0");
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let body = if body.is_null() {
            axum::body::Body::empty()
        } else {
            axum::body::Body::from(body.to_string())
        };
        let response = router
            .clone()
            .oneshot(request.body(body).unwrap())
            .await
            .unwrap();
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test]
    async fn a_paired_plugin_reports_its_sequence_and_the_live_view_shows_it() {
        use crate::server::remote_audit::{AuditOutcome, AuditRecord};
        let (_dir, state, router) = served();
        let (status, capabilities) = send(
            &router,
            "GET",
            "/api/sync/v1/capabilities",
            Some(TOKEN),
            serde_json::Value::Null,
        )
        .await;
        assert_eq!(status, 200, "{capabilities}");
        assert!(capabilities["data"]["capabilities"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!(SEQUENCE_STATUS_CAPABILITY)));

        let body = serde_json::json!({
            "session_id": Uuid::new_v4(), "reported_at_ms": 100, "running": true,
            "operation": "M 31 › Imaging › Smart Exposure",
            "activity": { "kind": "exposing", "target_name": "M 31", "exposure_seconds": 300.0, "completed": 3, "total": 30 },
        });
        let (status, _) = send(
            &router,
            "POST",
            "/api/sync/v1/sequence-status",
            Some("wrong-key-long-enough-to-pass-too"),
            body.clone(),
        )
        .await;
        assert!(status == 401 || status == 403, "{status}");
        assert!(state.sync_sequences.get("observatory").is_none());

        let (status, answer) = send(
            &router,
            "POST",
            "/api/sync/v1/sequence-status",
            Some(TOKEN),
            body,
        )
        .await;
        assert_eq!(status, 200, "{answer}");
        assert_eq!(answer["data"]["accepted"], true);

        state.remote_audit.record(
            "observatory",
            AuditAction::Apply,
            AuditOutcome::Ok,
            AuditRecord {
                operation: Some("merge"),
                ..Default::default()
            },
        );
        let catalog = state.get_database("observatory").unwrap();
        let live = SyncLive::for_catalog(&state, &catalog, now_ms()).unwrap();
        assert!(live.last_contact_ms.is_some());
        assert_eq!(live.plugin_version.as_deref(), Some("1.5.0.0"));
        assert_eq!(
            live.latest_action.unwrap().operation.as_deref(),
            Some("merge")
        );
        let sequence = live.sequence.unwrap();
        assert_eq!(
            sequence.activity.unwrap().target_name.as_deref(),
            Some("M 31")
        );
        assert!(!live.sequence_stale);
    }

    #[test]
    fn an_older_report_never_replaces_a_newer_one() {
        let statuses = SyncSequenceStatuses::default();
        let session = Uuid::new_v4();
        assert!(statuses.record("rig", report(session, 200).into_status(1).unwrap()));
        assert!(!statuses.record("rig", report(session, 100).into_status(2).unwrap()));
        assert_eq!(statuses.get("rig").unwrap().reported_at_ms, 200);
        // A late report from an earlier run never replaces the current one.
        let earlier = Uuid::new_v4();
        assert!(!statuses.record("rig", report(earlier, 150).into_status(3).unwrap()));
        assert_eq!(statuses.get("rig").unwrap().session_id, session);
        let next = Uuid::new_v4();
        assert!(statuses.record("rig", report(next, 300).into_status(4).unwrap()));
        assert_eq!(statuses.get("rig").unwrap().session_id, next);
        // Once the stored report is stale, an older clock is accepted.
        let late = report(Uuid::new_v4(), 10)
            .into_status(4 + MIN_FRESH_MS + 1)
            .unwrap();
        assert!(statuses.record("rig", late));
        assert!(statuses.get("other").is_none());
    }
}
