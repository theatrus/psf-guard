//! Shared processing defaults and explicit database overrides. Legacy
//! registrations retain their choices; newly added databases inherit.
use super::{
    api::ApiResponse,
    handlers::{require_database_management_allowed, require_registry_path, AppError},
    state::AppState,
};
use crate::db_registry::{DbRegistry, ProcessingDefaults, ProcessingOverrides};
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize)]
pub struct DatabaseProcessing {
    id: String,
    name: String,
    overrides: ProcessingOverrides,
    effective: ProcessingDefaults,
}
#[derive(Serialize)]
pub struct ProcessingSettings {
    defaults: ProcessingDefaults,
    databases: Vec<DatabaseProcessing>,
}
fn snapshot(registry: &DbRegistry) -> ProcessingSettings {
    ProcessingSettings {
        defaults: registry.processing_defaults,
        databases: registry
            .databases
            .iter()
            .map(|entry| DatabaseProcessing {
                id: entry.id.clone(),
                name: entry.name.clone(),
                overrides: entry.processing_overrides(),
                effective: ProcessingDefaults {
                    analyze_new_frames: entry.analyze_new_frames,
                    pair_calibrated_copies: entry.pair_calibrated_copies,
                    scan_calibrated_copies: entry.scan_calibrated_copies,
                },
            })
            .collect(),
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateProcessing {
    /// Absent edits shared defaults; present edits only this registration.
    pub(crate) database_id: Option<String>,
    /// Only included fields change. JSON null restores inheritance.
    pub(crate) values: std::collections::BTreeMap<String, Option<bool>>,
}
pub async fn get(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<ProcessingSettings>>, AppError> {
    let registry = match state.registry_path.read().unwrap().clone() {
        Some(path) => DbRegistry::load_or_init(&path)
            .map_err(|error| AppError::InternalError(error.to_string()))?,
        None => DbRegistry::default(),
    };
    Ok(Json(ApiResponse::success(snapshot(&registry))))
}
pub async fn update(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateProcessing>,
) -> Result<Json<ApiResponse<ProcessingSettings>>, AppError> {
    Ok(Json(ApiResponse::success(save(&state, request).await?)))
}
pub(crate) async fn save(
    state: &Arc<AppState>,
    request: UpdateProcessing,
) -> Result<ProcessingSettings, AppError> {
    require_database_management_allowed(state)?;
    let path = require_registry_path(state)?;
    let _guard = state.registry_write.lock().await;
    let mut registry = DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    for key in request.values.keys() {
        if !matches!(
            key.as_str(),
            "analyze_new_frames" | "pair_calibrated_copies" | "scan_calibrated_copies"
        ) {
            return Err(AppError::BadRequest(format!(
                "Unknown processing setting: {key}"
            )));
        }
    }
    if let Some(id) = &request.database_id {
        let entry = registry
            .databases
            .iter_mut()
            .find(|entry| &entry.id == id)
            .ok_or(AppError::NotFound)?;
        let mut own = entry.processing_overrides();
        for (key, value) in request.values {
            match key.as_str() {
                "analyze_new_frames" => own.analyze_new_frames = value,
                "pair_calibrated_copies" => own.pair_calibrated_copies = value,
                "scan_calibrated_copies" => own.scan_calibrated_copies = value,
                _ => unreachable!(),
            }
        }
        entry.processing_overrides = Some(own);
    } else {
        for (key, value) in request.values {
            let value = value.ok_or_else(|| {
                AppError::BadRequest("A shared default must be on or off.".into())
            })?;
            match key.as_str() {
                "analyze_new_frames" => registry.processing_defaults.analyze_new_frames = value,
                "pair_calibrated_copies" => {
                    registry.processing_defaults.pair_calibrated_copies = value
                }
                "scan_calibrated_copies" => {
                    registry.processing_defaults.scan_calibrated_copies = value
                }
                _ => unreachable!(),
            }
        }
    }
    for entry in &mut registry.databases {
        entry.resolve_processing(registry.processing_defaults);
    }
    registry
        .save(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    // Publish under the same write lock as persistence so a concurrent save
    // cannot leave live settings older than the saved registry.
    for entry in &registry.databases {
        if let Some(ctx) = state.get_database(&entry.id) {
            let start = entry.analyze_new_frames && !ctx.analyze_new_frames.is_on();
            ctx.analyze_new_frames.set(entry.analyze_new_frames);
            ctx.pair_calibrated_copies.set(entry.pair_calibrated_copies);
            ctx.scan_calibrated_copies.set(entry.scan_calibrated_copies);
            if start {
                super::quality_arrival::note_arrival(state, &entry.id);
            }
        }
    }
    Ok(snapshot(&registry))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let mut registry = DbRegistry::default();
        for id in ["legacy", "inherited"] {
            let file = dir.path().join(format!("{id}.sqlite"));
            drop(crate::ts_schema::create_fresh_db(&file).unwrap());
            let entry = registry
                .add(
                    id.into(),
                    file.to_string_lossy().into(),
                    vec![dir.path().to_string_lossy().into_owned()],
                    Some(id.into()),
                )
                .unwrap();
            let _ = entry;
        }
        registry.databases[0].processing_overrides = None;
        let path = dir.path().join("registry.json");
        registry.save(&path).unwrap();
        let state = Arc::new(
            AppState::from_databases(
                registry.databases,
                dir.path().join("cache").to_string_lossy().into_owned(),
                crate::cli::PregenerationConfig::default(),
            )
            .unwrap(),
        );
        state.set_registry_path(Some(path));
        state.set_allow_database_management(true);
        (dir, state)
    }
    fn change(database: Option<&str>, values: &[(&str, Option<bool>)]) -> UpdateProcessing {
        UpdateProcessing {
            database_id: database.map(String::from),
            values: values
                .iter()
                .map(|(key, value)| (key.to_string(), *value))
                .collect(),
        }
    }
    #[tokio::test]
    async fn defaults_override_reset_and_reload_preserve_scope_and_legacy_choices() {
        let (_dir, state) = fixture();
        save(
            &state,
            change(None, &[("pair_calibrated_copies", Some(false))]),
        )
        .await
        .unwrap();
        assert!(state
            .get_database("legacy")
            .unwrap()
            .pair_calibrated_copies
            .is_on());
        assert!(!state
            .get_database("inherited")
            .unwrap()
            .pair_calibrated_copies
            .is_on());
        save(
            &state,
            change(Some("inherited"), &[("pair_calibrated_copies", Some(true))]),
        )
        .await
        .unwrap();
        assert!(state
            .get_database("inherited")
            .unwrap()
            .pair_calibrated_copies
            .is_on());
        // Reset one legacy choice without changing its other explicit values.
        save(
            &state,
            change(Some("legacy"), &[("pair_calibrated_copies", None)]),
        )
        .await
        .unwrap();
        let path = require_registry_path(&state).unwrap();
        let registry = DbRegistry::load_or_init(&path).unwrap();
        let legacy = registry.find("legacy").unwrap();
        assert!(!legacy.pair_calibrated_copies);
        assert_eq!(legacy.processing_overrides().pair_calibrated_copies, None);
        assert_eq!(
            legacy.processing_overrides().analyze_new_frames,
            Some(false)
        );
        assert!(registry.find("inherited").unwrap().pair_calibrated_copies);
        let restarted = AppState::from_databases(
            registry.databases,
            path.parent()
                .unwrap()
                .join("restart-cache")
                .to_string_lossy()
                .into_owned(),
            crate::cli::PregenerationConfig::default(),
        )
        .unwrap();
        assert!(!restarted
            .get_database("legacy")
            .unwrap()
            .pair_calibrated_copies
            .is_on());
        assert!(restarted
            .get_database("inherited")
            .unwrap()
            .pair_calibrated_copies
            .is_on());
    }
    #[tokio::test]
    async fn rejects_unknown_fields_missing_databases_and_ungated_saves_without_writing() {
        let (_dir, state) = fixture();
        let path = require_registry_path(&state).unwrap();
        let before = std::fs::read(&path).unwrap();
        for request in [
            change(None, &[("unknown", Some(true))]),
            change(Some("removed"), &[("pair_calibrated_copies", None)]),
            change(None, &[("pair_calibrated_copies", None)]),
        ] {
            assert!(save(&state, request).await.is_err());
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
        state.set_allow_database_management(false);
        assert!(matches!(
            save(
                &state,
                change(None, &[("pair_calibrated_copies", Some(false))])
            )
            .await,
            Err(AppError::Forbidden(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }
    #[tokio::test]
    async fn concurrent_partial_defaults_and_legacy_api_saves_keep_explicit_overrides() {
        let (_dir, state) = fixture();
        let (a, b) = tokio::join!(
            save(
                &state,
                change(None, &[("pair_calibrated_copies", Some(false))])
            ),
            save(
                &state,
                change(None, &[("scan_calibrated_copies", Some(true))])
            )
        );
        a.unwrap();
        b.unwrap();
        let response = super::super::handlers::update_calibrated_copies(
            State(state.clone()),
            super::super::extract::DbContext(state.get_database("inherited").unwrap()),
            Json(super::super::handlers::CalibratedCopiesRequest {
                pair: Some(true),
                scan_calibrated: None,
            }),
        )
        .await;
        assert!(response.is_ok());
        save(
            &state,
            change(None, &[("scan_calibrated_copies", Some(false))]),
        )
        .await
        .unwrap();
        let registry = DbRegistry::load_or_init(&require_registry_path(&state).unwrap()).unwrap();
        assert!(!registry.processing_defaults.pair_calibrated_copies);
        assert!(!registry.processing_defaults.scan_calibrated_copies);
        assert_eq!(
            registry
                .find("inherited")
                .unwrap()
                .processing_overrides()
                .pair_calibrated_copies,
            Some(true)
        );
        assert!(state
            .get_database("inherited")
            .unwrap()
            .pair_calibrated_copies
            .is_on());
        assert!(!state
            .get_database("inherited")
            .unwrap()
            .scan_calibrated_copies
            .is_on());
    }
}
