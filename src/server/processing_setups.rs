//! Named processing setups: save, apply, import, and export the parameters of
//! the stack processing editors, shared across every configured database.
//!
//! Role enforcement is the standard middleware rule: GET is open to viewers,
//! and the mutating verbs land in `requires_write`, so only an editor (or an
//! open local server) can change setups. Settings are validated by
//! deserializing into the exact types the build endpoints consume, then stored
//! in that canonical form — a setup that saves is a setup the pipeline can
//! parse. Parameter ranges are still the build endpoints' business.

use axum::{
    extract::{Path, State},
    Json,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use crate::processing_setups::{
    ProcessingSetupKind, ProcessingSetupRecord, ProcessingSetupsRegistry,
};
use crate::server::{
    api::ApiResponse,
    handlers::{require_registry_path, AppError},
    state::AppState,
};

#[derive(Debug, Serialize)]
pub struct ProcessingSetupsResponse {
    pub schema_version: u32,
    pub setups: Vec<ProcessingSetupRecord>,
}

#[derive(Debug, Deserialize)]
pub struct SaveSetupRequest {
    pub name: String,
    pub kind: ProcessingSetupKind,
    pub settings: serde_json::Value,
}

#[derive(Debug, Deserialize)]
pub struct ImportSetupsRequest {
    /// The export format: the registry file itself.
    pub schema_version: u32,
    pub setups: Vec<ImportedSetup>,
}

#[derive(Debug, Deserialize)]
pub struct ImportedSetup {
    pub name: String,
    pub kind: ProcessingSetupKind,
    pub settings: serde_json::Value,
    // Timestamps in an export are informational; imports are stamped fresh.
    #[serde(default)]
    #[allow(dead_code)]
    pub created_unix_seconds: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub updated_unix_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ImportSetupsResponse {
    pub imported: usize,
    pub replaced: usize,
    pub setups: Vec<ProcessingSetupRecord>,
}

/// Shape-check settings against the type the matching build endpoint
/// deserializes, and return the canonical form (unknown fields dropped).
fn canonical_settings(
    kind: ProcessingSetupKind,
    settings: serde_json::Value,
) -> Result<serde_json::Value, AppError> {
    let canonical = match kind {
        ProcessingSetupKind::View => serde_json::from_value::<
            crate::server::stack_preview::stretch::StackViewProcessingRequest,
        >(settings)
        .map_err(|error| bad_settings(kind, &error))
        .and_then(|parsed| {
            serde_json::to_value(parsed).map_err(|error| bad_settings(kind, &error))
        })?,
        ProcessingSetupKind::Color => {
            // Every pipeline field is optional, so a foreign object would
            // otherwise parse as an empty pipeline and save silently. A
            // non-empty object must mention at least one pipeline field.
            const COLOR_KEYS: [&str; 4] = [
                "background_extraction",
                "input_deconvolutions",
                "input_stretches",
                "output_stretches",
            ];
            if let Some(object) = settings.as_object()
                && !object.is_empty()
                && !object.keys().any(|key| COLOR_KEYS.contains(&key.as_str()))
            {
                return Err(AppError::BadRequest(
                        "These settings do not describe color processing: no pipeline field is present \
                         (background_extraction, input_deconvolutions, input_stretches, \
                         output_stretches)"
                            .into(),
                    ));
            }
            serde_json::from_value::<crate::server::stack_preview::color::StackColorProcessing>(
                settings,
            )
            .map_err(|error| bad_settings(kind, &error))
            .and_then(|parsed| {
                serde_json::to_value(parsed).map_err(|error| bad_settings(kind, &error))
            })?
        }
    };
    Ok(canonical)
}

fn bad_settings(kind: ProcessingSetupKind, error: &dyn std::fmt::Display) -> AppError {
    AppError::BadRequest(format!(
        "These settings do not describe {} processing: {error}",
        kind.label()
    ))
}

fn setups_path(state: &AppState) -> Result<PathBuf, AppError> {
    let registry = require_registry_path(state)?;
    Ok(ProcessingSetupsRegistry::path_for_database_registry(
        &registry,
    ))
}

fn load_registry(path: &std::path::Path) -> Result<ProcessingSetupsRegistry, AppError> {
    ProcessingSetupsRegistry::load(path).map_err(|error| AppError::InternalError(error.to_string()))
}

fn save_registry(
    registry: &ProcessingSetupsRegistry,
    path: &std::path::Path,
) -> Result<(), AppError> {
    registry
        .save(path)
        .map_err(|error| AppError::InternalError(error.to_string()))
}

/// GET /api/processing-setups — also the export document: saving this
/// response verbatim yields a file the import endpoint accepts.
pub async fn list_setups(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<ProcessingSetupsResponse>>, AppError> {
    // A server without a persistent registry has nowhere to keep setups; an
    // empty list reads better in every panel than an error banner.
    let registry = match setups_path(&state) {
        Ok(path) => load_registry(&path)?,
        Err(_) => ProcessingSetupsRegistry::default(),
    };
    Ok(Json(ApiResponse::success(ProcessingSetupsResponse {
        schema_version: registry.schema_version,
        setups: registry.setups,
    })))
}

/// POST /api/processing-setups — create or replace one setup by name.
pub async fn save_setup(
    State(state): State<Arc<AppState>>,
    Json(request): Json<SaveSetupRequest>,
) -> Result<Json<ApiResponse<ProcessingSetupRecord>>, AppError> {
    let path = setups_path(&state)?;
    let settings = canonical_settings(request.kind, request.settings)?;
    let now = chrono::Utc::now().timestamp();
    let record = ProcessingSetupRecord {
        name: request.name,
        kind: request.kind,
        settings,
        created_unix_seconds: now,
        updated_unix_seconds: now,
    };
    let saved_name = record.name.clone();

    let _guard = state.processing_setups_write.lock().unwrap();
    let mut registry = load_registry(&path)?;
    registry
        .upsert(record)
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    save_registry(&registry, &path)?;
    let saved = registry
        .find(&saved_name)
        .cloned()
        .ok_or_else(|| AppError::InternalError("Saved setup did not persist".into()))?;
    Ok(Json(ApiResponse::success(saved)))
}

/// DELETE /api/processing-setups/{name}
pub async fn delete_setup(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<ApiResponse<ProcessingSetupsResponse>>, AppError> {
    let path = setups_path(&state)?;
    let _guard = state.processing_setups_write.lock().unwrap();
    let mut registry = load_registry(&path)?;
    if !registry.remove(&name) {
        return Err(AppError::NotFoundMessage(format!(
            "No processing setup is named {name}"
        )));
    }
    save_registry(&registry, &path)?;
    Ok(Json(ApiResponse::success(ProcessingSetupsResponse {
        schema_version: registry.schema_version,
        setups: registry.setups,
    })))
}

/// POST /api/processing-setups/import — merge an exported file into the
/// registry, replacing same-named setups. All-or-nothing: one setup that does
/// not validate fails the whole import, so a partial file never lands quietly.
pub async fn import_setups(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ImportSetupsRequest>,
) -> Result<Json<ApiResponse<ImportSetupsResponse>>, AppError> {
    if request.schema_version != crate::processing_setups::CURRENT_SCHEMA_VERSION {
        return Err(AppError::BadRequest(format!(
            "This import has schema {} but this server understands {}",
            request.schema_version,
            crate::processing_setups::CURRENT_SCHEMA_VERSION
        )));
    }
    let path = setups_path(&state)?;
    let now = chrono::Utc::now().timestamp();

    let mut records = Vec::with_capacity(request.setups.len());
    for setup in request.setups {
        let settings =
            canonical_settings(setup.kind, setup.settings).map_err(|error| match error {
                AppError::BadRequest(message) => {
                    AppError::BadRequest(format!("Setup \"{}\": {message}", setup.name))
                }
                other => other,
            })?;
        records.push(ProcessingSetupRecord {
            name: setup.name,
            kind: setup.kind,
            settings,
            created_unix_seconds: now,
            updated_unix_seconds: now,
        });
    }

    let _guard = state.processing_setups_write.lock().unwrap();
    let mut registry = load_registry(&path)?;
    let mut imported = 0usize;
    let mut replaced = 0usize;
    let mut staged = registry.clone();
    for record in records {
        if staged
            .upsert(record)
            .map_err(|error| AppError::BadRequest(error.to_string()))?
        {
            replaced += 1;
        } else {
            imported += 1;
        }
    }
    save_registry(&staged, &path)?;
    registry = staged;
    Ok(Json(ApiResponse::success(ImportSetupsResponse {
        imported,
        replaced,
        setups: registry.setups,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_settings_validate_against_the_stretch_request_type() {
        let good = serde_json::json!({
            "model": { "type": "auto-mtf", "target_median": 0.25, "shadows_clip": -2.8 },
            "color_strategy": "linked",
        });
        let canonical = canonical_settings(ProcessingSetupKind::View, good).unwrap();
        assert_eq!(canonical["model"]["type"], "auto-mtf");

        let wrong_kind = serde_json::json!({ "model": { "type": "no-such-stretch" } });
        assert!(canonical_settings(ProcessingSetupKind::View, wrong_kind).is_err());
    }

    #[test]
    fn color_settings_validate_against_the_pipeline_type() {
        let good = serde_json::json!({
            "background_extraction": null,
            "input_stretches": {
                "ha": [{ "model": { "type": "auto-mtf", "target_median": 0.2,
                                     "shadows_clip": -2.8 },
                         "color_strategy": "linked" }],
            },
            "output_stretches": [],
        });
        let canonical = canonical_settings(ProcessingSetupKind::Color, good).unwrap();
        assert!(canonical["input_stretches"]["ha"].is_array());

        // A view-shaped payload is not a color pipeline. Every pipeline
        // field defaults, so without the key guard this would save silently
        // as an empty pipeline.
        let view_shaped = serde_json::json!({ "model": { "type": "identity" } });
        assert!(canonical_settings(ProcessingSetupKind::Color, view_shaped).is_err());

        // An explicitly empty object stays a valid "no processing" pipeline.
        assert!(canonical_settings(ProcessingSetupKind::Color, serde_json::json!({})).is_ok());
    }

    #[test]
    fn unknown_fields_are_dropped_rather_than_stored() {
        let padded = serde_json::json!({
            "model": { "type": "identity" },
            "color_strategy": "linked",
            "someday_field": true,
        });
        let canonical = canonical_settings(ProcessingSetupKind::View, padded).unwrap();
        assert!(canonical.get("someday_field").is_none());
    }
}
