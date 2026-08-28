//! RC-Astro post-processing on stacked linear FITS: BlurXTerminator,
//! NoiseXTerminator, and StarXTerminator through their standalone CLI.
//!
//! This is a reversible view-processing tier like deconvolution: the
//! integration artifact is never touched, results cache under their own
//! identity, and "Revert processing" simply stops requesting them. Star
//! removal keeps both halves — the starless image becomes the processed
//! linear source and the stars image is stored beside it — so each can be
//! stretched on its own.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;

use axum::Json;
use seiza_stacking::{
    write_processed_image_fits_f32, ExternalParameterValue, ExternalToolRequest,
    ExternalToolSchema, LinearImage, RcAstroCli,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::server::api::ApiResponse;
use crate::server::handlers::AppError;
use crate::server::state::AppState;

pub(super) const RC_ASTRO_CACHE_VERSION: u32 = 1;

/// The order steps run when several are enabled: sharpen the linear data,
/// denoise it, then separate the stars.
pub const RC_ASTRO_STEP_ORDER: [&str; 3] = ["bxt", "nxt", "sxt"];

/// One tool run with its parameter values, keyed by schema parameter name.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct RcAstroStep {
    pub tool: String,
    #[serde(default)]
    pub parameters: BTreeMap<String, ExternalParameterValue>,
}

/// The requested chain of tool runs.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct RcAstroProcessing {
    pub steps: Vec<RcAstroStep>,
}

impl RcAstroProcessing {
    /// Refuse a request the run loop would fail on anyway, with a clearer
    /// message and before anything is hashed or cached.
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("RC-Astro processing lists no steps".into());
        }
        let mut seen = Vec::new();
        for step in &self.steps {
            if !RC_ASTRO_STEP_ORDER.contains(&step.tool.as_str()) {
                return Err(format!("unknown RC-Astro tool {:?}", step.tool));
            }
            if seen.contains(&step.tool.as_str()) {
                return Err(format!("RC-Astro tool {:?} is listed twice", step.tool));
            }
            seen.push(step.tool.as_str());
        }
        Ok(())
    }

    /// Steps in canonical run order, regardless of request order.
    fn ordered(&self) -> Vec<&RcAstroStep> {
        let mut steps: Vec<&RcAstroStep> = self.steps.iter().collect();
        steps.sort_by_key(|step| {
            RC_ASTRO_STEP_ORDER
                .iter()
                .position(|tool| *tool == step.tool)
                .unwrap_or(usize::MAX)
        });
        steps
    }
}

/// One executed step, reported back to the client.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RcAstroStepResult {
    pub tool: String,
    pub name: String,
    pub ml_version: Option<i64>,
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// The whole chain's outcome, reported back to the client.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StackRcAstroResult {
    pub cli_version: String,
    pub steps: Vec<RcAstroStepResult>,
    /// Whether a stars image was produced and stored beside the result.
    pub has_stars: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CachedRcAstro {
    schema_version: u32,
    rc_astro_id: String,
    config: RcAstroProcessing,
    result: StackRcAstroResult,
}

/// What `GET /api/tools/rc-astro` reports: whether the CLI is installed and
/// each tool's live schema, so the UI can build its controls from the same
/// contract the run will use.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RcAstroCapabilities {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(default)]
    pub tools: Vec<ExternalToolSchema>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn cli() -> Option<RcAstroCli> {
    // A service unit often runs with a minimal PATH; PSF_GUARD_RC_ASTRO
    // names the executable directly.
    let located = match std::env::var_os("PSF_GUARD_RC_ASTRO") {
        Some(path) if !path.is_empty() => Some(RcAstroCli::with_executable(PathBuf::from(path))),
        _ => RcAstroCli::locate(),
    };
    located.map(|cli| cli.with_host(format!("psf-guard-{}", env!("CARGO_PKG_VERSION"))))
}

/// Probe the installed CLI and every tool's schema. Spawns short-lived
/// processes; callers wrap it in `spawn_blocking`.
pub fn probe_capabilities() -> RcAstroCapabilities {
    let Some(cli) = cli() else {
        return RcAstroCapabilities {
            available: false,
            executable: None,
            tools: Vec::new(),
            error: None,
        };
    };
    let mut tools = Vec::new();
    let mut error = None;
    for tool in RC_ASTRO_STEP_ORDER {
        match cli.tool_schema(tool) {
            Ok(schema) => tools.push(schema),
            Err(problem) => error = Some(problem.to_string()),
        }
    }
    RcAstroCapabilities {
        available: !tools.is_empty(),
        executable: Some(cli.executable().display().to_string()),
        tools,
        error,
    }
}

/// GET /api/tools/rc-astro
pub async fn get_rc_astro_capabilities(
    axum::extract::State(_state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<ApiResponse<RcAstroCapabilities>>, AppError> {
    let capabilities = tokio::task::spawn_blocking(probe_capabilities)
        .await
        .map_err(|error| AppError::InternalError(format!("RC-Astro probe failed: {error}")))?;
    Ok(Json(ApiResponse::success(capabilities)))
}

/// Read the schemas the requested steps need, or say which tool refused.
/// Spawns short-lived processes; callers wrap it in `spawn_blocking`.
pub(super) fn schemas_for(
    config: &RcAstroProcessing,
) -> Result<Vec<(String, ExternalToolSchema)>, String> {
    let cli = cli().ok_or_else(|| "rc-astro is not installed on this server".to_string())?;
    let mut schemas = Vec::new();
    for step in config.ordered() {
        let schema = cli
            .tool_schema(&step.tool)
            .map_err(|error| error.to_string())?;
        if !schema.licensed {
            return Err(format!(
                "{} is not licensed on this server{}",
                schema.name,
                schema
                    .license_message
                    .as_deref()
                    .map(|message| format!(" ({message})"))
                    .unwrap_or_default()
            ));
        }
        schemas.push((step.tool.clone(), schema));
    }
    Ok(schemas)
}

/// The cache identity of one processing chain over one source. Includes
/// each tool's CLI and model versions: an upgrade changes the output for
/// identical inputs, so it must change the identity too.
pub(super) fn rc_astro_cache_id(
    database_id: &str,
    source_key: &str,
    source_revision: &str,
    config: &RcAstroProcessing,
    schemas: &[(String, ExternalToolSchema)],
) -> Result<String, AppError> {
    let mut ordered = config.ordered().into_iter().cloned().collect::<Vec<_>>();
    // The canonical order is the identity: two requests that run the same
    // steps the same way cache as one.
    let encoded = serde_json::to_vec(&ordered).map_err(|error| {
        AppError::InternalError(format!("Failed to encode RC-Astro request: {error}"))
    })?;
    ordered.clear();
    let mut hasher = Sha256::new();
    hasher.update(database_id.as_bytes());
    hasher.update(source_key.as_bytes());
    hasher.update(source_revision.as_bytes());
    hasher.update(RC_ASTRO_CACHE_VERSION.to_le_bytes());
    hasher.update(&encoded);
    for (tool, schema) in schemas {
        hasher.update(tool.as_bytes());
        hasher.update(schema.cli_version.as_bytes());
        hasher.update(schema.ml_version.unwrap_or(0).to_le_bytes());
    }
    let mut id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(id)
}

pub(super) fn rc_astro_dir(cache_root: &FsPath, rc_astro_id: &str) -> PathBuf {
    cache_root
        .join("stack-previews")
        .join("rc-astro")
        .join(rc_astro_id)
}

fn rc_astro_manifest_path(cache_root: &FsPath, rc_astro_id: &str) -> PathBuf {
    rc_astro_dir(cache_root, rc_astro_id).join("manifest.json")
}

pub(super) fn rc_astro_fits_path(cache_root: &FsPath, rc_astro_id: &str) -> PathBuf {
    rc_astro_dir(cache_root, rc_astro_id).join("processed.fits")
}

pub(super) fn rc_astro_stars_path(cache_root: &FsPath, rc_astro_id: &str) -> PathBuf {
    rc_astro_dir(cache_root, rc_astro_id).join("stars.fits")
}

pub(super) struct RcAstroOutcome {
    /// The processed linear image — the starless one when stars were
    /// removed.
    pub image: LinearImage,
    /// The stars image, when star removal kept one.
    pub stars: Option<LinearImage>,
    pub result: StackRcAstroResult,
}

/// Run (or reuse) the chain over one linear image. The processed FITS and
/// the stars FITS land in the cache directory; the returned images feed the
/// stretch renders directly.
pub(super) fn apply_rc_astro(
    cache_root: &FsPath,
    rc_astro_id: &str,
    config: &RcAstroProcessing,
    schemas: &[(String, ExternalToolSchema)],
    image: &LinearImage,
    reference_headers: &[(String, seiza_fits::HeaderValue)],
) -> Result<RcAstroOutcome, String> {
    let fits = rc_astro_fits_path(cache_root, rc_astro_id);
    let stars_fits = rc_astro_stars_path(cache_root, rc_astro_id);
    let manifest_path = rc_astro_manifest_path(cache_root, rc_astro_id);

    if let Ok(bytes) = std::fs::read(&manifest_path)
        && let Ok(cached) = serde_json::from_slice::<CachedRcAstro>(&bytes)
        && cached.schema_version == RC_ASTRO_CACHE_VERSION
        && cached.rc_astro_id == rc_astro_id
        && &cached.config == config
        && fits.is_file()
        && (!cached.result.has_stars || stars_fits.is_file())
    {
        let processed = crate::image_io::open_linear_frame(&fits)
            .map_err(|error| error.to_string())?
            .image;
        let stars = if cached.result.has_stars {
            Some(
                crate::image_io::open_linear_frame(&stars_fits)
                    .map_err(|error| error.to_string())?
                    .image,
            )
        } else {
            None
        };
        return Ok(RcAstroOutcome {
            image: processed,
            stars,
            result: cached.result,
        });
    }

    let cli = cli().ok_or_else(|| "rc-astro is not installed on this server".to_string())?;
    let mut current = image.clone();
    let mut stars: Option<LinearImage> = None;
    let mut steps = Vec::new();
    let mut cli_version = String::new();
    for step in config.ordered() {
        let (_, schema) = schemas
            .iter()
            .find(|(tool, _)| tool == &step.tool)
            .ok_or_else(|| format!("no schema for RC-Astro tool {:?}", step.tool))?;
        cli_version = schema.cli_version.clone();
        let request = ExternalToolRequest {
            tool: step.tool.clone(),
            parameters: step
                .parameters
                .iter()
                .map(|(name, value)| (name.clone(), *value))
                .collect(),
            device: None,
        };
        tracing::info!(
            "RC-Astro {}: running {} on {}x{}x{}",
            rc_astro_id,
            schema.name,
            current.width,
            current.height,
            current.channels
        );
        let processed = cli
            .process_image(
                schema,
                &request,
                &current,
                reference_headers,
                None,
                &mut |_| {},
            )
            .map_err(|error| error.to_string())?;
        if let Some(step_stars) = processed.stars {
            stars = Some(step_stars);
        }
        steps.push(RcAstroStepResult {
            tool: step.tool.clone(),
            name: schema.name.clone(),
            ml_version: schema.ml_version,
            device: processed.device.clone(),
            warnings: processed.warnings.clone(),
        });
        current = processed.image;
    }

    let result = StackRcAstroResult {
        cli_version,
        steps,
        has_stars: stars.is_some(),
    };

    let parent = fits
        .parent()
        .ok_or_else(|| "RC-Astro FITS path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = fits.with_extension(format!("{}.tmp.fits", std::process::id()));
    write_processed_image_fits_f32(&temporary, &current, reference_headers, &rc_astro_cards())
        .map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &fits).map_err(|error| error.to_string())?;
    if let Some(stars_image) = &stars {
        let temporary = stars_fits.with_extension(format!("{}.tmp.fits", std::process::id()));
        write_processed_image_fits_f32(
            &temporary,
            stars_image,
            reference_headers,
            &rc_astro_cards(),
        )
        .map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &stars_fits).map_err(|error| error.to_string())?;
    }
    super::stretch::write_json_atomic(
        &manifest_path,
        &CachedRcAstro {
            schema_version: RC_ASTRO_CACHE_VERSION,
            rc_astro_id: rc_astro_id.into(),
            config: config.clone(),
            result: result.clone(),
        },
    )?;

    Ok(RcAstroOutcome {
        image: current,
        stars,
        result,
    })
}

fn rc_astro_cards() -> Vec<seiza_fits::WriteHeaderCard> {
    vec![
        seiza_fits::WriteHeaderCard::new(
            "SEIZARCA",
            seiza_fits::HeaderValue::String("RC-ASTRO".into()),
        )
        .with_comment("processed by RC-Astro standalone tools"),
        seiza_fits::WriteHeaderCard::new(
            "SEIZATRF",
            seiza_fits::HeaderValue::String("LINEAR".into()),
        )
        .with_comment("linear sample transfer"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(tool: &str) -> RcAstroStep {
        RcAstroStep {
            tool: tool.into(),
            parameters: BTreeMap::new(),
        }
    }

    fn schema(tool: &str, cli_version: &str, ml_version: i64) -> ExternalToolSchema {
        serde_json::from_value(serde_json::json!({
            "schema_version": 6,
            "cli_version": cli_version,
            "key": tool,
            "name": format!("RC-Astro {tool}"),
            "ml_version": ml_version,
            "licensed": true,
            "license_message": null,
            "parameters": [],
        }))
        .unwrap()
    }

    #[test]
    fn validation_refuses_unknown_and_duplicate_tools_and_empty_chains() {
        assert!(RcAstroProcessing { steps: vec![] }.validate().is_err());
        assert!(RcAstroProcessing {
            steps: vec![step("gxt")]
        }
        .validate()
        .is_err());
        assert!(RcAstroProcessing {
            steps: vec![step("sxt"), step("sxt")]
        }
        .validate()
        .is_err());
        assert!(RcAstroProcessing {
            steps: vec![step("bxt"), step("sxt")]
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn the_cache_identity_ignores_request_order_but_tracks_versions() {
        let schemas = vec![("bxt".to_string(), schema("bxt", "2.6.6", 4))];
        let forward = rc_astro_cache_id(
            "db",
            "mono:job:0",
            "rev",
            &RcAstroProcessing {
                steps: vec![step("bxt"), step("sxt")],
            },
            &schemas,
        )
        .unwrap();
        let reversed = rc_astro_cache_id(
            "db",
            "mono:job:0",
            "rev",
            &RcAstroProcessing {
                steps: vec![step("sxt"), step("bxt")],
            },
            &schemas,
        )
        .unwrap();
        assert_eq!(forward, reversed);

        let upgraded = rc_astro_cache_id(
            "db",
            "mono:job:0",
            "rev",
            &RcAstroProcessing {
                steps: vec![step("bxt"), step("sxt")],
            },
            &[("bxt".to_string(), schema("bxt", "2.7.0", 5))],
        )
        .unwrap();
        assert_ne!(forward, upgraded);
    }

    #[test]
    fn steps_run_in_canonical_order() {
        let config = RcAstroProcessing {
            steps: vec![step("sxt"), step("bxt"), step("nxt")],
        };
        let ordered: Vec<&str> = config
            .ordered()
            .iter()
            .map(|step| step.tool.as_str())
            .collect();
        assert_eq!(ordered, vec!["bxt", "nxt", "sxt"]);
    }

    /// A stand-in rc-astro: copies input to output, writes a stars sidecar
    /// when asked, and counts its invocations so the cache test can prove a
    /// second apply never spawned it.
    #[cfg(unix)]
    fn fake_rc_astro(directory: &std::path::Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = directory.join("rc-astro");
        let script = format!(
            r#"#!/bin/sh
echo run >> "{count}"
out=""
input=""
stars=0
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift 2 ;;
    --stars) stars=1; shift ;;
    --host|--depth|--device) shift 2 ;;
    --*|sxt|bxt|nxt) shift ;;
    *) input="$1"; shift ;;
  esac
done
cp "$input" "$out"
echo '{{"event":"status","phase":"complete","output":"'"$out"'"}}'
if [ "$stars" = 1 ]; then
  sidecar="${{out%.fits}}-stars.fits"
  cp "$input" "$sidecar"
  echo '{{"event":"status","phase":"complete","output":"'"$sidecar"'"}}'
fi
"#,
            count = directory.join("invocations").display()
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    #[test]
    fn a_chain_runs_once_and_the_cache_serves_the_second_apply() {
        let tool_dir = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let executable = fake_rc_astro(tool_dir.path());
        // Point the module at the stand-in for this test. Env mutation is
        // process-wide, so keep the whole flow inside one test.
        unsafe { std::env::set_var("PSF_GUARD_RC_ASTRO", &executable) };

        let config = RcAstroProcessing {
            steps: vec![RcAstroStep {
                tool: "sxt".into(),
                parameters: BTreeMap::from([(
                    "stars".to_string(),
                    ExternalParameterValue::Bool(true),
                )]),
            }],
        };
        let mut schema = schema("sxt", "2.6.6", 11);
        schema.parameters = vec![serde_json::from_value(serde_json::json!({
            "name": "stars",
            "flag": "--stars",
            "label": "Generate Star Image",
            "description": "",
            "kind": {"type": "bool", "default": false},
        }))
        .unwrap()];
        let schemas = vec![("sxt".to_string(), schema)];
        let image = LinearImage::new(3, 2, 1, vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0]).unwrap();

        let first = apply_rc_astro(
            cache.path(),
            "a".repeat(64).as_str(),
            &config,
            &schemas,
            &image,
            &[],
        )
        .unwrap();
        assert!(first.result.has_stars);
        assert!(first.stars.is_some());
        for (processed, original) in first.image.data.iter().zip(&image.data) {
            assert!((processed - original).abs() < 1e-2);
        }

        let second = apply_rc_astro(
            cache.path(),
            "a".repeat(64).as_str(),
            &config,
            &schemas,
            &image,
            &[],
        )
        .unwrap();
        assert!(second.result.has_stars);
        unsafe { std::env::remove_var("PSF_GUARD_RC_ASTRO") };

        let invocations =
            std::fs::read_to_string(tool_dir.path().join("invocations")).unwrap_or_default();
        assert_eq!(
            invocations.lines().count(),
            1,
            "second apply must be a cache hit"
        );
    }
}
