//! Named processing setups: save, apply, import, and export the parameters of
//! the stack processing editors, shared across every configured database.
//!
//! Role enforcement is the standard middleware rule: GET is open to viewers,
//! and the mutating verbs land in `requires_write`, so only an editor (or an
//! open local server) can change setups. Settings are validated by
//! deserializing into the exact types the build endpoints consume, then stored
//! in that canonical form — a setup that saves is a setup the pipeline can
//! parse. Parameter ranges are still the build endpoints' business.
//!
//! The Color setup marked as the default supplies the pipeline every new
//! color preview starts from; [`default_color_processing`] is the one place
//! the server reads it, for a person's builds and automatic refreshes alike.

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
    stack_preview::color::StackColorProcessing,
    state::AppState,
};

#[derive(Debug, Serialize)]
pub struct ProcessingSetupsResponse {
    pub schema_version: u32,
    pub setups: Vec<ProcessingSetupRecord>,
    /// The Color setup new color previews start from. `None` when no setup
    /// is marked, and the built-in pipeline applies.
    pub default_color: Option<String>,
    /// The pipeline new color previews start from, with a stretch for every
    /// channel: the marked setup's, or the built-in one.
    pub default_color_processing: StackColorProcessing,
}

impl ProcessingSetupsResponse {
    fn of(registry: ProcessingSetupsRegistry) -> Self {
        Self {
            schema_version: registry.schema_version,
            default_color: registry
                .default_color_setup()
                .map(|setup| setup.name.clone()),
            default_color_processing: default_color_processing_in(&registry),
            setups: registry.setups,
        }
    }
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
    // Imports are stamped fresh. The timestamps only tell the seeded
    // default setup, which has none until someone saves over it.
    #[serde(default)]
    pub created_unix_seconds: Option<i64>,
    #[serde(default)]
    pub updated_unix_seconds: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ImportSetupsResponse {
    pub imported: usize,
    pub replaced: usize,
    /// Seeded default setups nobody edited, left out because this install
    /// already has a setup of that name.
    pub skipped: usize,
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
            if let Some(config) = &parsed.rc_astro {
                config.validate().map_err(AppError::BadRequest)?;
            }
            serde_json::to_value(parsed).map_err(|error| bad_settings(kind, &error))
        })?,
        ProcessingSetupKind::Color => {
            // Every pipeline field is optional, so a foreign object would
            // otherwise parse as an empty pipeline and save silently. A
            // non-empty object must mention at least one pipeline field.
            const COLOR_KEYS: [&str; 6] = [
                "input_processing",
                "background_extraction",
                "input_deconvolutions",
                "input_rc_astro",
                "input_stretches",
                "output_stretches",
            ];
            if let Some(object) = settings.as_object()
                && !object.is_empty()
                && !object.keys().any(|key| COLOR_KEYS.contains(&key.as_str()))
            {
                return Err(AppError::BadRequest(
                        "These settings do not describe color processing: no pipeline field is present \
                         (background_extraction, input_deconvolutions, input_rc_astro, input_stretches, \
                         output_stretches)"
                            .into(),
                    ));
            }
            serde_json::from_value::<crate::server::stack_preview::color::StackColorProcessing>(
                settings,
            )
            .map_err(|error| bad_settings(kind, &error))
            .and_then(|parsed| {
                for config in parsed.input_rc_astro.values() {
                    config.validate().map_err(AppError::BadRequest)?;
                }
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

/// The built-in color pipeline in the canonical form a saved setup has.
fn builtin_color_settings() -> serde_json::Value {
    let settings = serde_json::from_str(crate::processing_setups::DEFAULT_COLOR_SETTINGS)
        .expect("the built-in color pipeline is JSON");
    canonical_settings(ProcessingSetupKind::Color, settings)
        .expect("the built-in color pipeline is a color pipeline")
}

fn builtin_color_processing() -> StackColorProcessing {
    serde_json::from_value(builtin_color_settings()).expect("the built-in color pipeline parses")
}

/// The registry with the seeded default Color setup when it has never had a
/// default. The seed is written with the next change, not here.
fn load_registry(path: &std::path::Path) -> Result<ProcessingSetupsRegistry, AppError> {
    let mut registry = ProcessingSetupsRegistry::load(path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    registry.seed_default_color(builtin_color_settings());
    Ok(registry)
}

/// The pipeline new color previews start from: the marked Color setup's,
/// with a stretch for every channel, or the built-in one when no setup is
/// marked. A setup saved from some channels gives the others the stretch
/// it applies to all of its own, when it applies one stretch to several,
/// and the built-in stretch otherwise; the Setups bar fills channels the
/// same way when it applies a setup to a card.
fn default_color_processing_in(registry: &ProcessingSetupsRegistry) -> StackColorProcessing {
    let builtin = builtin_color_processing();
    let Some(setup) = registry.default_color_setup() else {
        return builtin;
    };
    let mut processing = match serde_json::from_value::<StackColorProcessing>(
        setup.settings.clone(),
    ) {
        Ok(processing) => processing,
        Err(error) => {
            tracing::warn!(
                    "the default Color setup \"{}\" does not parse, so new color previews use the built-in pipeline: {error}",
                    setup.name
                );
            return builtin;
        }
    };
    let own = processing
        .input_stretches
        .values()
        .map(|stages| serde_json::to_value(stages).unwrap_or_default())
        .collect::<Vec<_>>();
    let shared = (own.len() > 1 && own.iter().all(|stages| *stages == own[0]))
        .then(|| processing.input_stretches.values().next().cloned())
        .flatten();
    for (role, stages) in builtin.input_stretches {
        processing
            .input_stretches
            .entry(role)
            .or_insert_with(|| shared.clone().unwrap_or(stages));
    }
    processing
}

/// The pipeline new color previews start from, read from the setups file
/// now. A server without a database registry, or whose setups file cannot
/// be read, uses the built-in pipeline.
pub fn default_color_processing(state: &AppState) -> StackColorProcessing {
    let Ok(path) = setups_path(state) else {
        return builtin_color_processing();
    };
    match load_registry(&path) {
        Ok(registry) => default_color_processing_in(&registry),
        Err(error) => {
            tracing::warn!(
                "could not read the default Color setup, so new color previews use the built-in pipeline: {error:?}"
            );
            builtin_color_processing()
        }
    }
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
    // empty list with the built-in color pipeline reads better in every
    // panel than an error banner.
    let registry = match setups_path(&state) {
        Ok(path) => load_registry(&path)?,
        Err(_) => ProcessingSetupsRegistry::default(),
    };
    Ok(Json(ApiResponse::success(ProcessingSetupsResponse::of(
        registry,
    ))))
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
        other_fields: Default::default(),
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
    if !registry
        .remove(&name)
        .map_err(|error| AppError::Conflict(error.to_string()))?
    {
        return Err(AppError::NotFoundMessage(format!(
            "No processing setup is named {name}"
        )));
    }
    save_registry(&registry, &path)?;
    Ok(Json(ApiResponse::success(ProcessingSetupsResponse::of(
        registry,
    ))))
}

/// PUT /api/processing-setups/{name}/default — make a Color setup the one
/// new color previews start from, a person's and automatic ones alike.
pub async fn mark_default_setup(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> Result<Json<ApiResponse<ProcessingSetupsResponse>>, AppError> {
    let path = setups_path(&state)?;
    let _guard = state.processing_setups_write.lock().unwrap();
    let mut registry = load_registry(&path)?;
    if registry.find(&name).is_none() {
        return Err(AppError::NotFoundMessage(format!(
            "No processing setup is named {name}"
        )));
    }
    registry
        .mark_default_color(&name)
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    save_registry(&registry, &path)?;
    Ok(Json(ApiResponse::success(ProcessingSetupsResponse::of(
        registry,
    ))))
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
        let untouched_seed =
            setup.created_unix_seconds == Some(0) && setup.updated_unix_seconds == Some(0);
        let settings =
            canonical_settings(setup.kind, setup.settings).map_err(|error| match error {
                AppError::BadRequest(message) => {
                    AppError::BadRequest(format!("Setup \"{}\": {message}", setup.name))
                }
                other => other,
            })?;
        let record = ProcessingSetupRecord {
            name: setup.name,
            kind: setup.kind,
            settings,
            created_unix_seconds: now,
            updated_unix_seconds: now,
            other_fields: Default::default(),
        };
        records.push((record, untouched_seed));
    }

    let _guard = state.processing_setups_write.lock().unwrap();
    let mut registry = load_registry(&path)?;
    let mut imported = 0usize;
    let mut replaced = 0usize;
    let mut skipped = 0usize;
    let mut staged = registry.clone();
    for (record, untouched_seed) in records {
        // Another install's built-in pipeline must not replace this one's
        // own setup of the same name, such as an edited "Default".
        if untouched_seed && staged.find(&record.name).is_some() {
            skipped += 1;
            continue;
        }
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
        skipped,
        setups: registry.setups,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setups_keep_rc_astro_steps_without_requiring_installed_tools() {
        let chain = serde_json::json!({"steps": [{"tool": "bxt", "parameters": {"sharpen": 0.4}}, {"tool": "sxt"}]});
        let view = canonical_settings(
            ProcessingSetupKind::View,
            serde_json::json!({"model": {"type": "identity"}, "rc_astro": chain}),
        )
        .unwrap();
        assert_eq!(view["rc_astro"]["steps"][0]["parameters"]["sharpen"], 0.4);
        let color = canonical_settings(
            ProcessingSetupKind::Color,
            serde_json::json!({"input_rc_astro": {"red": chain}}),
        )
        .unwrap();
        assert_eq!(color["input_rc_astro"]["red"]["steps"][1]["tool"], "sxt");
        for chain in [
            serde_json::json!({"steps": []}),
            serde_json::json!({"steps": [{"tool":"invalid"}]}),
            serde_json::json!({"steps": [{"tool":"bxt"},{"tool":"bxt"}]}),
        ] {
            assert!(canonical_settings(
                ProcessingSetupKind::View,
                serde_json::json!({"model": {"type": "identity"}, "rc_astro": chain})
            )
            .is_err());
            assert!(canonical_settings(
                ProcessingSetupKind::Color,
                serde_json::json!({"input_rc_astro": {"red": chain}})
            )
            .is_err());
        }
    }

    #[test]
    fn color_setups_keep_whether_inputs_follow_the_channel_cards() {
        let following = canonical_settings(
            ProcessingSetupKind::Color,
            serde_json::json!({"input_processing": "channels"}),
        )
        .unwrap();
        assert_eq!(following["input_processing"], "channels");
        // A setup saved before the choice existed keeps its own tools.
        let own = canonical_settings(
            ProcessingSetupKind::Color,
            serde_json::json!({"input_deconvolutions": {}}),
        )
        .unwrap();
        assert!(own.get("input_processing").is_none(), "{own}");
    }

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

    /// Equal as JSON, with every number compared as a float: `1` in the
    /// shared file and `1.0` from an `f64` field are the same value.
    fn same_json(left: &serde_json::Value, right: &serde_json::Value) -> bool {
        use serde_json::Value;
        match (left, right) {
            (Value::Number(left), Value::Number(right)) => left.as_f64() == right.as_f64(),
            (Value::Array(left), Value::Array(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(left, right)| same_json(left, right))
            }
            (Value::Object(left), Value::Object(right)) => {
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right.get(key).is_some_and(|other| same_json(value, other))
                    })
            }
            _ => left == right,
        }
    }

    #[test]
    fn the_built_in_pipeline_saves_as_it_is_written() {
        // The file the frontend's defaultColorProcessing is checked against
        // changes on the way into a setup only where a stored pipeline
        // always differs: it leaves out empty maps, and Seiza's background
        // configuration adds an empty list of protected regions.
        let mut written: serde_json::Value =
            serde_json::from_str(crate::processing_setups::DEFAULT_COLOR_SETTINGS).unwrap();
        let mut canonical = builtin_color_settings();
        let object = written.as_object_mut().unwrap();
        assert_eq!(
            object.remove("input_deconvolutions"),
            Some(serde_json::json!({}))
        );
        assert_eq!(object.remove("input_rc_astro"), Some(serde_json::json!({})));
        let config = canonical["background_extraction"]["config"]
            .as_object_mut()
            .unwrap();
        assert_eq!(
            config.remove("protected_regions"),
            Some(serde_json::json!([]))
        );
        assert!(same_json(&written, &canonical), "{written}\n{canonical}");
    }

    fn stretch(target_median: f64) -> serde_json::Value {
        serde_json::json!([{
            "model": { "type": "auto-mtf", "target_median": target_median, "shadows_clip": -2.8 },
            "color_strategy": "linked",
        }])
    }

    fn registry_with_default(settings: serde_json::Value) -> ProcessingSetupsRegistry {
        let mut registry = ProcessingSetupsRegistry::default();
        registry
            .upsert(ProcessingSetupRecord {
                name: "Faint".into(),
                kind: ProcessingSetupKind::Color,
                settings: canonical_settings(ProcessingSetupKind::Color, settings).unwrap(),
                created_unix_seconds: 1,
                updated_unix_seconds: 1,
                other_fields: Default::default(),
            })
            .unwrap();
        registry.seed_default_color(builtin_color_settings());
        registry.mark_default_color("Faint").unwrap();
        registry
    }

    #[test]
    fn the_marked_setup_gives_every_channel_a_stretch() {
        use crate::server::stack_preview::color::StackColorRole::{Blue, Ha, Luminance, Oiii, Sii};
        let median = |processing: &StackColorProcessing, role| {
            serde_json::to_value(&processing.input_stretches[&role]).unwrap()[0]["model"]
                ["target_median"]
                .as_f64()
                .unwrap()
        };
        // Saved from an SHO card with one stretch for all three: every other
        // channel gets that stretch too.
        let shared = default_color_processing_in(&registry_with_default(serde_json::json!({
            "background_extraction": null,
            "input_stretches": { "ha": stretch(0.1), "oiii": stretch(0.1), "sii": stretch(0.1) },
        })));
        assert_eq!(shared.input_stretches.len(), 7);
        assert_eq!(median(&shared, Luminance), 0.1);
        assert!(shared.background_extraction.is_none());

        // One stretch per channel: the channels it names keep theirs and
        // the rest get the built-in stretch.
        let own = default_color_processing_in(&registry_with_default(serde_json::json!({
            "input_stretches": { "ha": stretch(0.1), "oiii": stretch(0.3) },
        })));
        assert_eq!(median(&own, Ha), 0.1);
        assert_eq!(median(&own, Oiii), 0.3);
        assert_eq!(median(&own, Sii), 0.2);
        assert_eq!(median(&own, Blue), 0.2);
    }

    #[test]
    fn without_a_marked_setup_new_color_previews_use_the_built_in_pipeline() {
        let builtin = serde_json::to_value(builtin_color_processing()).unwrap();
        let seeded = default_color_processing_in(&{
            let mut registry = ProcessingSetupsRegistry::default();
            registry.seed_default_color(builtin_color_settings());
            registry
        });
        assert_eq!(serde_json::to_value(seeded).unwrap(), builtin);
        let unmarked = ProcessingSetupsRegistry {
            default_color: Some("Gone".into()),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(default_color_processing_in(&unmarked)).unwrap(),
            builtin
        );
    }

    fn state_with_setups(directory: &std::path::Path) -> (Arc<AppState>, PathBuf) {
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        let registry = directory.join("config.json");
        *state.registry_path.write().unwrap() = Some(registry.clone());
        (
            state,
            ProcessingSetupsRegistry::path_for_database_registry(&registry),
        )
    }

    async fn listed(state: &Arc<AppState>) -> ProcessingSetupsResponse {
        list_setups(State(Arc::clone(state)))
            .await
            .unwrap()
            .0
            .data
            .unwrap()
    }

    fn save_request(
        name: &str,
        kind: ProcessingSetupKind,
        settings: serde_json::Value,
    ) -> Json<SaveSetupRequest> {
        Json(SaveSetupRequest {
            name: name.into(),
            kind,
            settings,
        })
    }

    #[tokio::test]
    async fn the_list_shows_the_seeded_default_without_writing_it() {
        let directory = tempfile::tempdir().unwrap();
        let (state, path) = state_with_setups(directory.path());
        let listing = listed(&state).await;
        assert_eq!(listing.default_color.as_deref(), Some("Default"));
        assert_eq!(listing.setups.len(), 1);
        assert_eq!(listing.setups[0].settings, builtin_color_settings());
        assert_eq!(
            serde_json::to_value(&listing.default_color_processing).unwrap(),
            serde_json::to_value(builtin_color_processing()).unwrap()
        );
        assert!(!path.exists(), "a read wrote the setups file");
        // The automatic refresh reads the same pipeline.
        assert_eq!(
            serde_json::to_value(default_color_processing(&state)).unwrap(),
            serde_json::to_value(&listing.default_color_processing).unwrap()
        );

        // The next change writes the seed with it.
        let _ = save_setup(
            State(Arc::clone(&state)),
            save_request(
                "Bright",
                ProcessingSetupKind::View,
                serde_json::json!({"model": {"type": "identity"}}),
            ),
        )
        .await
        .unwrap();
        let stored = load_registry(&path).unwrap();
        assert_eq!(stored.default_color.as_deref(), Some("Default"));
        assert!(stored.find("Default").is_some());
    }

    #[tokio::test]
    async fn marking_a_setup_default_changes_what_new_color_previews_start_from() {
        let directory = tempfile::tempdir().unwrap();
        let (state, _) = state_with_setups(directory.path());
        let _ = save_setup(
            State(Arc::clone(&state)),
            save_request(
                "Faint",
                ProcessingSetupKind::Color,
                serde_json::json!({ "input_stretches": { "red": stretch(0.1), "green": stretch(0.1) } }),
            ),
        )
        .await
        .unwrap();
        let _ = save_setup(
            State(Arc::clone(&state)),
            save_request(
                "Bright",
                ProcessingSetupKind::View,
                serde_json::json!({"model": {"type": "identity"}}),
            ),
        )
        .await
        .unwrap();

        let refused = mark_default_setup(State(Arc::clone(&state)), Path("Bright".into())).await;
        assert!(matches!(refused, Err(AppError::BadRequest(_))));
        let missing = mark_default_setup(State(Arc::clone(&state)), Path("Absent".into())).await;
        assert!(matches!(missing, Err(AppError::NotFoundMessage(_))));

        let listing = mark_default_setup(State(Arc::clone(&state)), Path("faint".into()))
            .await
            .unwrap()
            .0
            .data
            .unwrap();
        assert_eq!(listing.default_color.as_deref(), Some("Faint"));
        let automatic = default_color_processing(&state);
        assert_eq!(automatic.input_stretches.len(), 7);
        assert!(
            automatic.color_calibration.is_none(),
            "the setup's own, unset"
        );
        assert_eq!(
            serde_json::to_value(
                &automatic.input_stretches
                    [&crate::server::stack_preview::color::StackColorRole::Luminance]
            )
            .unwrap(),
            stretch(0.1)
        );

        // The seed is an ordinary setup again once it is not the default.
        let _ = delete_setup(State(Arc::clone(&state)), Path("Default".into()))
            .await
            .unwrap();
        assert_eq!(
            listed(&state).await.setups.len(),
            2,
            "the seed does not come back"
        );
    }

    fn view_settings() -> serde_json::Value {
        serde_json::json!({"model": {"type": "identity"}})
    }

    #[tokio::test]
    async fn the_default_cannot_be_deleted_or_turned_into_a_view_setup() {
        let directory = tempfile::tempdir().unwrap();
        let (state, _) = state_with_setups(directory.path());
        let deleted = delete_setup(State(Arc::clone(&state)), Path("Default".into())).await;
        assert!(matches!(deleted, Err(AppError::Conflict(_))));

        // Saving a Color pipeline under its name edits it.
        let _ = save_setup(
            State(Arc::clone(&state)),
            save_request(
                "default",
                ProcessingSetupKind::Color,
                serde_json::json!({ "background_extraction": null, "output_stretches": [] }),
            ),
        )
        .await
        .unwrap();
        let listing = listed(&state).await;
        assert_eq!(listing.default_color.as_deref(), Some("Default"));
        assert!(listing
            .default_color_processing
            .background_extraction
            .is_none());
        let replaced = save_setup(
            State(Arc::clone(&state)),
            save_request("default", ProcessingSetupKind::View, view_settings()),
        )
        .await;
        assert!(matches!(replaced, Err(AppError::BadRequest(_))));
    }

    #[tokio::test]
    async fn a_view_setup_named_default_moves_the_seed_and_it_stays_the_default() {
        let directory = tempfile::tempdir().unwrap();
        let (state, path) = state_with_setups(directory.path());
        // An install with no setups file yet, as before 0.13.
        let _ = save_setup(
            State(Arc::clone(&state)),
            save_request("Default", ProcessingSetupKind::View, view_settings()),
        )
        .await
        .unwrap();
        let listing = listed(&state).await;
        assert_eq!(listing.default_color.as_deref(), Some("Default color"));
        assert_eq!(
            serde_json::to_value(&listing.default_color_processing).unwrap(),
            serde_json::to_value(builtin_color_processing()).unwrap()
        );
        let stored = ProcessingSetupsRegistry::load(&path).unwrap();
        assert_eq!(stored.default_color.as_deref(), Some("Default color"));

        // An older export with a view "Default" imports the same way.
        let directory = tempfile::tempdir().unwrap();
        let (state, _) = state_with_setups(directory.path());
        let _ = import_setups(
            State(Arc::clone(&state)),
            Json(
                serde_json::from_value(serde_json::json!({
                    "schema_version": 1,
                    "setups": [{ "name": "Default", "kind": "view", "settings": view_settings(),
                                 "created_unix_seconds": 5, "updated_unix_seconds": 5 }],
                }))
                .unwrap(),
            ),
        )
        .await
        .unwrap();
        let listing = listed(&state).await;
        assert_eq!(listing.default_color.as_deref(), Some("Default color"));
        assert_eq!(listing.setups.len(), 2);
    }

    async fn import(
        state: &Arc<AppState>,
        setups: serde_json::Value,
    ) -> Result<Json<ApiResponse<ImportSetupsResponse>>, AppError> {
        import_setups(
            State(Arc::clone(state)),
            Json(
                serde_json::from_value(serde_json::json!({
                    "schema_version": 1,
                    "setups": setups,
                    // A list response saved as a file carries these.
                    "default_color": "Other",
                }))
                .unwrap(),
            ),
        )
        .await
    }

    #[tokio::test]
    async fn an_import_edits_the_default_by_name_and_keeps_the_mark() {
        let directory = tempfile::tempdir().unwrap();
        let (state, _) = state_with_setups(directory.path());
        let _ = import(
            &state,
            serde_json::json!([
                { "name": "Other", "kind": "color", "settings": { "output_stretches": [] } },
                { "name": "DEFAULT", "kind": "color", "settings": { "background_extraction": null } },
            ]),
        )
        .await
        .unwrap();
        let listing = listed(&state).await;
        assert_eq!(
            listing.default_color.as_deref(),
            Some("Default"),
            "an import does not move the mark"
        );
        assert!(listing
            .default_color_processing
            .background_extraction
            .is_none());

        // Once edited, an import cannot put a view setup in its place.
        let view_over_default = import(
            &state,
            serde_json::json!([{ "name": "Default", "kind": "view", "settings": view_settings() }]),
        )
        .await;
        assert!(matches!(view_over_default, Err(AppError::BadRequest(_))));
    }

    #[tokio::test]
    async fn another_installs_unedited_default_does_not_replace_this_ones() {
        let directory = tempfile::tempdir().unwrap();
        let (state, _) = state_with_setups(directory.path());
        let _ = save_setup(
            State(Arc::clone(&state)),
            save_request(
                "Default",
                ProcessingSetupKind::Color,
                serde_json::json!({ "background_extraction": null }),
            ),
        )
        .await
        .unwrap();
        // An export from an install whose seed nobody edited.
        let seed = serde_json::json!({
            "name": "Default", "kind": "color", "settings": builtin_color_settings(),
            "created_unix_seconds": 0, "updated_unix_seconds": 0,
        });
        let result = import(
            &state,
            serde_json::json!([seed, { "name": "Other", "kind": "color", "settings": {} }]),
        )
        .await
        .unwrap()
        .0
        .data
        .unwrap();
        assert_eq!(
            (result.imported, result.replaced, result.skipped),
            (1, 0, 1)
        );
        assert!(listed(&state)
            .await
            .default_color_processing
            .background_extraction
            .is_none());

        // An install without that name takes it, with fresh timestamps.
        let directory = tempfile::tempdir().unwrap();
        let (state, _) = state_with_setups(directory.path());
        let mut renamed = seed.clone();
        renamed["name"] = "Built in elsewhere".into();
        let result = import(&state, serde_json::json!([renamed]))
            .await
            .unwrap()
            .0
            .data
            .unwrap();
        assert_eq!((result.imported, result.skipped), (1, 0));
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
