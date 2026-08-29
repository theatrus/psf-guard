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

pub(super) const RC_ASTRO_CACHE_VERSION: u32 = 2;

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
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<ApiResponse<RcAstroCapabilities>>, AppError> {
    let mut capabilities = tokio::task::spawn_blocking(probe_capabilities)
        .await
        .map_err(|error| AppError::InternalError(format!("RC-Astro probe failed: {error}")))?;
    // Every viewer may see which tools exist; the license line names the
    // licensee and the executable names a server path, so those stay with
    // the operator role.
    for tool in &mut capabilities.tools {
        tool.license_message = None;
    }
    if !state.database_management_allowed() {
        capabilities.executable = None;
    }
    Ok(Json(ApiResponse::success(capabilities)))
}

/// A short-lived schema cache: every stretch apply needs the schemas to
/// compute its cache identity, and spawning three probe processes per
/// request adds latency for an answer that only changes on a CLI upgrade.
static SCHEMA_CACHE: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, (std::time::Instant, ExternalToolSchema)>>,
> = std::sync::LazyLock::new(Default::default);
const SCHEMA_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(60);

fn cached_tool_schema(cli: &RcAstroCli, tool: &str) -> Result<ExternalToolSchema, String> {
    if let Ok(cache) = SCHEMA_CACHE.lock()
        && let Some((read_at, schema)) = cache.get(tool)
        && read_at.elapsed() < SCHEMA_CACHE_TTL
    {
        return Ok(schema.clone());
    }
    let schema = cli.tool_schema(tool).map_err(|error| error.to_string())?;
    if let Ok(mut cache) = SCHEMA_CACHE.lock() {
        cache.insert(
            tool.to_string(),
            (std::time::Instant::now(), schema.clone()),
        );
    }
    Ok(schema)
}

/// Read the schemas the requested steps need, or say which tool refused.
/// Spawns short-lived processes; callers wrap it in `spawn_blocking`.
pub(super) fn schemas_for(
    config: &RcAstroProcessing,
) -> Result<Vec<(String, ExternalToolSchema)>, String> {
    let cli = cli().ok_or_else(|| "rc-astro is not installed on this server".to_string())?;
    let mut schemas = Vec::new();
    for step in config.ordered() {
        let schema = cached_tool_schema(&cli, &step.tool)?;
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

/// Canonicalize a request against the live schemas: refuse unknown,
/// GUI-only, and out-of-range parameters with a clear message before
/// anything runs, and coerce a whole-number float (which JSON delivers as
/// an integer) to its float form. Coercion must happen before hashing:
/// `1` and `1.0` are the same request and must share one cache identity.
pub(super) fn normalize_against_schemas(
    config: &RcAstroProcessing,
    schemas: &[(String, ExternalToolSchema)],
) -> Result<RcAstroProcessing, String> {
    let mut steps = Vec::new();
    for step in config.ordered() {
        let (_, schema) = schemas
            .iter()
            .find(|(tool, _)| tool == &step.tool)
            .ok_or_else(|| format!("no schema for RC-Astro tool {:?}", step.tool))?;
        let mut parameters = BTreeMap::new();
        for (name, value) in &step.parameters {
            let parameter = schema
                .parameters
                .iter()
                .find(|parameter| &parameter.name == name)
                .ok_or_else(|| format!("{}: unknown parameter {name:?}", schema.name))?;
            if parameter.flag.is_none() {
                return Err(format!(
                    "{}: parameter {name:?} cannot be set from the CLI",
                    schema.name
                ));
            }
            let value = match (&parameter.kind, value) {
                (
                    seiza_stacking::ExternalParameterKind::Float { min, max, .. },
                    ExternalParameterValue::Int(raw),
                ) => {
                    let coerced = *raw as f64;
                    if coerced < *min || coerced > *max {
                        return Err(format!(
                            "{}: {name} = {coerced} is outside [{min}, {max}]",
                            schema.name
                        ));
                    }
                    ExternalParameterValue::Float(coerced)
                }
                (
                    seiza_stacking::ExternalParameterKind::Float { min, max, .. },
                    ExternalParameterValue::Float(v),
                ) => {
                    if !v.is_finite() || v < min || v > max {
                        return Err(format!(
                            "{}: {name} = {v} is outside [{min}, {max}]",
                            schema.name
                        ));
                    }
                    ExternalParameterValue::Float(*v)
                }
                (
                    seiza_stacking::ExternalParameterKind::Int { min, max, .. },
                    ExternalParameterValue::Int(v),
                ) => {
                    if v < min || v > max {
                        return Err(format!(
                            "{}: {name} = {v} is outside [{min}, {max}]",
                            schema.name
                        ));
                    }
                    ExternalParameterValue::Int(*v)
                }
                // The mirror coercion: the schema hands int bounds back as
                // JSON numbers, so a UI clamping to them sends 2.0 for 2.
                (
                    seiza_stacking::ExternalParameterKind::Int { min, max, .. },
                    ExternalParameterValue::Float(real),
                ) => {
                    if !real.is_finite() || real.fract() != 0.0 {
                        return Err(format!(
                            "{}: {name} = {real} is not a whole number",
                            schema.name
                        ));
                    }
                    let v = *real as i64;
                    if v < *min || v > *max {
                        return Err(format!(
                            "{}: {name} = {v} is outside [{min}, {max}]",
                            schema.name
                        ));
                    }
                    ExternalParameterValue::Int(v)
                }
                (
                    seiza_stacking::ExternalParameterKind::Bool { .. },
                    ExternalParameterValue::Bool(v),
                ) => ExternalParameterValue::Bool(*v),
                (expected, provided) => {
                    return Err(format!(
                        "{}: parameter {name:?} expects {expected:?}, got {provided:?}",
                        schema.name
                    ));
                }
            };
            parameters.insert(name.clone(), value);
        }
        steps.push(RcAstroStep {
            tool: step.tool.clone(),
            parameters,
        });
    }
    Ok(RcAstroProcessing { steps })
}

/// The cache identities of one processing chain over one source: one id per
/// canonical-order prefix, ending with the whole chain's id.
///
/// Each step's id folds the previous id together with only that step's
/// configuration and tool versions, so a change to a later step leaves every
/// earlier id — and its cached artifact — untouched. Turning NoiseX on after
/// a BlurX run reuses the BlurX output instead of recomputing it. The base
/// includes the deconvolution identity because the chain processes the
/// deconvolved image, not the raw stack, and each step's CLI and model
/// versions because an upgrade changes the output for identical inputs.
pub(super) fn rc_astro_chain_ids(
    database_id: &str,
    source_key: &str,
    source_revision: &str,
    config: &RcAstroProcessing,
    schemas: &[(String, ExternalToolSchema)],
    deconvolution_id: Option<&str>,
) -> Result<Vec<String>, AppError> {
    let mut hasher = Sha256::new();
    hasher.update(database_id.as_bytes());
    hasher.update(source_key.as_bytes());
    hasher.update(source_revision.as_bytes());
    hasher.update(RC_ASTRO_CACHE_VERSION.to_le_bytes());
    hasher.update(b"deconvolution");
    hasher.update(deconvolution_id.unwrap_or("none").as_bytes());
    let mut previous = hasher.finalize();

    let mut ids = Vec::new();
    // The canonical order is the identity: two requests that run the same
    // steps the same way cache as one.
    for step in config.ordered() {
        let encoded = serde_json::to_vec(step).map_err(|error| {
            AppError::InternalError(format!("Failed to encode RC-Astro request: {error}"))
        })?;
        let (_, schema) = schemas
            .iter()
            .find(|(tool, _)| tool == &step.tool)
            .ok_or_else(|| {
                AppError::BadRequest(format!("no schema for RC-Astro tool {:?}", step.tool))
            })?;
        let mut hasher = Sha256::new();
        hasher.update(previous);
        hasher.update(&encoded);
        hasher.update(step.tool.as_bytes());
        hasher.update(schema.cli_version.as_bytes());
        hasher.update(schema.ml_version.unwrap_or(0).to_le_bytes());
        previous = hasher.finalize();
        let mut id = String::with_capacity(64);
        for byte in previous {
            write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
        }
        ids.push(id);
    }
    Ok(ids)
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

/// Mark a cache entry as recently useful, so age-based pruning keeps live
/// variants and drops abandoned ones.
fn refresh_manifest_mtime(manifest_path: &FsPath) {
    if let Ok(file) = std::fs::File::options().append(true).open(manifest_path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

/// Load one cached prefix, or say why it does not serve. A hit refreshes
/// the manifest mtime.
fn load_cached_prefix(
    cache_root: &FsPath,
    rc_astro_id: &str,
    prefix: &RcAstroProcessing,
) -> Option<(LinearImage, Option<LinearImage>, StackRcAstroResult)> {
    let fits = rc_astro_fits_path(cache_root, rc_astro_id);
    let stars_fits = rc_astro_stars_path(cache_root, rc_astro_id);
    let manifest_path = rc_astro_manifest_path(cache_root, rc_astro_id);
    let bytes = std::fs::read(&manifest_path).ok()?;
    let cached = serde_json::from_slice::<CachedRcAstro>(&bytes).ok()?;
    if cached.schema_version != RC_ASTRO_CACHE_VERSION
        || cached.rc_astro_id != rc_astro_id
        || &cached.config != prefix
        || !fits.is_file()
        || (cached.result.has_stars && !stars_fits.is_file())
    {
        return None;
    }
    refresh_manifest_mtime(&manifest_path);
    let processed = crate::image_io::open_linear_frame(&fits).ok()?.image;
    let stars = if cached.result.has_stars {
        Some(crate::image_io::open_linear_frame(&stars_fits).ok()?.image)
    } else {
        None
    };
    Some((processed, stars, cached.result))
}

/// Write one prefix's artifacts: the processed FITS, the stars FITS when one
/// exists, and the manifest naming what produced them.
fn persist_prefix(
    cache_root: &FsPath,
    rc_astro_id: &str,
    prefix: &RcAstroProcessing,
    image: &LinearImage,
    stars: Option<&LinearImage>,
    result: &StackRcAstroResult,
    reference_headers: &[(String, seiza_fits::HeaderValue)],
) -> Result<(), String> {
    let fits = rc_astro_fits_path(cache_root, rc_astro_id);
    let stars_fits = rc_astro_stars_path(cache_root, rc_astro_id);
    let parent = fits
        .parent()
        .ok_or_else(|| "RC-Astro FITS path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    // The temp name carries a process-wide counter besides the pid: two
    // writers within one process must never interleave into one temp file.
    static TEMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = format!(
        "{}-{}",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let temporary = fits.with_extension(format!("{unique}.tmp.fits"));
    write_processed_image_fits_f32(&temporary, image, reference_headers, &rc_astro_cards())
        .map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, &fits).map_err(|error| error.to_string())?;
    if let Some(stars_image) = stars {
        let temporary = stars_fits.with_extension(format!("{unique}.tmp.fits"));
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
        &rc_astro_manifest_path(cache_root, rc_astro_id),
        &CachedRcAstro {
            schema_version: RC_ASTRO_CACHE_VERSION,
            rc_astro_id: rc_astro_id.into(),
            config: prefix.clone(),
            result: result.clone(),
        },
    )
}

/// Run (or reuse) the chain over one linear image. Every canonical prefix
/// caches under its own identity from `rc_astro_chain_ids`, so this resumes
/// from the deepest prefix already on disk: changing or adding a later step
/// reruns only from that step. The final prefix's FITS files are what the
/// download handlers serve; the returned images feed the stretch renders
/// directly.
pub(super) fn apply_rc_astro(
    cache_root: &FsPath,
    chain_ids: &[String],
    config: &RcAstroProcessing,
    schemas: &[(String, ExternalToolSchema)],
    image: &LinearImage,
    reference_headers: &[(String, seiza_fits::HeaderValue)],
    on_progress: &mut dyn FnMut(&str, f32),
) -> Result<RcAstroOutcome, String> {
    let ordered = config.ordered();
    if chain_ids.len() != ordered.len() {
        return Err(format!(
            "RC-Astro chain lists {} ids for {} steps",
            chain_ids.len(),
            ordered.len()
        ));
    }
    let prefix_config = |end: usize| RcAstroProcessing {
        steps: ordered[..end].iter().map(|step| (*step).clone()).collect(),
    };

    // Resume from the deepest cached prefix — the whole chain when nothing
    // changed, an earlier step's output when only later steps did.
    let mut current = image.clone();
    let mut stars: Option<LinearImage> = None;
    let mut steps = Vec::new();
    let mut cli_version = String::new();
    let mut resume_at = 0;
    for end in (1..=ordered.len()).rev() {
        if let Some((cached_image, cached_stars, cached_result)) =
            load_cached_prefix(cache_root, &chain_ids[end - 1], &prefix_config(end))
        {
            // The served prefix refreshed its own mtime; refresh the ones
            // under it too. A chain the user keeps re-applying must keep
            // its intermediate artifacts alive through the age sweep, or
            // the first later-step retune after two weeks reruns the whole
            // chain — the recompute this cache layout exists to avoid.
            for id in &chain_ids[..end - 1] {
                refresh_manifest_mtime(&rc_astro_manifest_path(cache_root, id));
            }
            if end == ordered.len() {
                return Ok(RcAstroOutcome {
                    image: cached_image,
                    stars: cached_stars,
                    result: cached_result,
                });
            }
            current = cached_image;
            stars = cached_stars;
            cli_version = cached_result.cli_version.clone();
            steps = cached_result.steps;
            resume_at = end;
            break;
        }
    }

    let cli = cli().ok_or_else(|| "rc-astro is not installed on this server".to_string())?;
    let total_steps = ordered.len().max(1);
    for (index, step) in ordered.iter().enumerate().skip(resume_at) {
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
            chain_ids[index],
            schema.name,
            current.width,
            current.height,
            current.channels
        );
        on_progress(&schema.name, index as f32 / total_steps as f32);
        // The chain's overall fraction: completed steps plus this step's
        // own fraction, over the step count.
        let mut step_progress = |fraction: f32| {
            on_progress(&schema.name, (index as f32 + fraction) / total_steps as f32)
        };
        let processed = cli
            .process_image(
                schema,
                &request,
                &current,
                reference_headers,
                None,
                &mut step_progress,
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

        // Persist this prefix under its own identity: a later request that
        // shares it — the same steps with a new step added, or a later step
        // changed — resumes from here instead of rerunning the tools.
        let so_far = StackRcAstroResult {
            cli_version: cli_version.clone(),
            steps: steps.clone(),
            has_stars: stars.is_some(),
        };
        persist_prefix(
            cache_root,
            &chain_ids[index],
            &prefix_config(index + 1),
            &current,
            stars.as_ref(),
            &so_far,
            reference_headers,
        )?;
    }

    let result = StackRcAstroResult {
        cli_version,
        steps,
        has_stars: stars.is_some(),
    };

    Ok(RcAstroOutcome {
        image: current,
        stars,
        result,
    })
}

/// A chain result whose manifest has gone untouched this long is abandoned:
/// nobody re-applied those settings for two weeks. A reuse refreshes the
/// manifest's mtime, so live variants survive.
const RC_ASTRO_RETENTION: std::time::Duration = std::time::Duration::from_secs(14 * 24 * 3600);

/// Sweep abandoned chain results and orphaned temp files. Each entry can be
/// hundreds of megabytes (processed plus stars FITS), and version bumps
/// deliberately mint new identities, so the directory grows without this.
pub(super) fn prune_rc_astro_cache(cache_root: &FsPath) {
    let root = cache_root.join("stack-previews").join("rc-astro");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let stale = |path: &std::path::Path, retention: std::time::Duration| {
        std::fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > retention)
    };
    for entry in entries.flatten() {
        let directory = entry.path();
        if !directory.is_dir() {
            continue;
        }
        let manifest = directory.join("manifest.json");
        let remove = if manifest.is_file() {
            stale(&manifest, RC_ASTRO_RETENTION)
        } else {
            // No manifest means a write crashed part way: give a running
            // build a day, then sweep the debris.
            stale(&directory, std::time::Duration::from_secs(24 * 3600))
        };
        if remove {
            let _ = std::fs::remove_dir_all(&directory);
        }
    }
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
    fn the_cache_identity_ignores_request_order_but_tracks_versions_and_deconvolution() {
        let schemas = vec![
            ("bxt".to_string(), schema("bxt", "2.6.6", 4)),
            ("sxt".to_string(), schema("sxt", "2.6.6", 11)),
        ];
        let config = RcAstroProcessing {
            steps: vec![step("bxt"), step("sxt")],
        };
        let forward =
            rc_astro_chain_ids("db", "mono:job:0", "rev", &config, &schemas, None).unwrap();
        let reversed = rc_astro_chain_ids(
            "db",
            "mono:job:0",
            "rev",
            &RcAstroProcessing {
                steps: vec![step("sxt"), step("bxt")],
            },
            &schemas,
            None,
        )
        .unwrap();
        assert_eq!(forward, reversed);
        assert_eq!(forward.len(), 2);

        let upgraded = rc_astro_chain_ids(
            "db",
            "mono:job:0",
            "rev",
            &config,
            &[
                ("bxt".to_string(), schema("bxt", "2.7.0", 5)),
                ("sxt".to_string(), schema("sxt", "2.6.6", 11)),
            ],
            None,
        )
        .unwrap();
        assert_ne!(forward, upgraded);

        // The chain processes the deconvolved image, so the deconvolution
        // identity is part of this identity: without it, "BXT after 3px
        // deconvolution" and "BXT alone" would serve each other's pixels.
        let deconvolved =
            rc_astro_chain_ids("db", "mono:job:0", "rev", &config, &schemas, Some("abc123"))
                .unwrap();
        assert_ne!(forward, deconvolved);
    }

    #[test]
    fn a_later_step_change_leaves_earlier_prefix_identities_alone() {
        let schemas = vec![
            ("bxt".to_string(), schema("bxt", "2.6.6", 4)),
            ("nxt".to_string(), schema("nxt", "2.6.6", 3)),
            ("sxt".to_string(), schema("sxt", "2.6.6", 11)),
        ];
        let bxt_only = rc_astro_chain_ids(
            "db",
            "mono:job:0",
            "rev",
            &RcAstroProcessing {
                steps: vec![step("bxt")],
            },
            &schemas,
            None,
        )
        .unwrap();
        let with_nxt = rc_astro_chain_ids(
            "db",
            "mono:job:0",
            "rev",
            &RcAstroProcessing {
                steps: vec![step("bxt"), step("nxt")],
            },
            &schemas,
            None,
        )
        .unwrap();
        // Turning NoiseX on keeps the BlurX prefix identity, so the cached
        // BlurX artifact serves the longer chain.
        assert_eq!(bxt_only[0], with_nxt[0]);
        assert_ne!(with_nxt[0], with_nxt[1]);

        // Changing a NoiseX parameter changes only the NoiseX identity.
        let with_tuned_nxt = rc_astro_chain_ids(
            "db",
            "mono:job:0",
            "rev",
            &RcAstroProcessing {
                steps: vec![
                    step("bxt"),
                    RcAstroStep {
                        tool: "nxt".into(),
                        parameters: BTreeMap::from([(
                            "denoise".to_string(),
                            ExternalParameterValue::Float(0.7),
                        )]),
                    },
                ],
            },
            &schemas,
            None,
        )
        .unwrap();
        assert_eq!(with_nxt[0], with_tuned_nxt[0]);
        assert_ne!(with_nxt[1], with_tuned_nxt[1]);

        // An sxt-only chain shares nothing with a bxt-led one: its first
        // step is a different tool, so its first identity differs.
        let sxt_only = rc_astro_chain_ids(
            "db",
            "mono:job:0",
            "rev",
            &RcAstroProcessing {
                steps: vec![step("sxt")],
            },
            &schemas,
            None,
        )
        .unwrap();
        assert_ne!(sxt_only[0], bxt_only[0]);
    }

    #[test]
    fn normalization_coerces_whole_number_floats_and_refuses_bad_parameters() {
        let mut sxt = schema("sxt", "2.6.6", 11);
        sxt.parameters = vec![
            serde_json::from_value(serde_json::json!({
                "name": "overlap", "flag": "--overlap", "label": "Overlap",
                "description": "", "kind": {"type": "float", "default": 0.2, "min": 0.0, "max": 2.0},
            }))
            .unwrap(),
            serde_json::from_value(serde_json::json!({
                "name": "csep", "flag": null, "label": "Color Separation",
                "description": "", "kind": {"type": "bool", "default": false},
            }))
            .unwrap(),
        ];
        let schemas = vec![("sxt".to_string(), sxt)];

        // JSON delivers 1.0 as 1; the same request must hash identically.
        let with_int = RcAstroProcessing {
            steps: vec![RcAstroStep {
                tool: "sxt".into(),
                parameters: BTreeMap::from([(
                    "overlap".to_string(),
                    ExternalParameterValue::Int(1),
                )]),
            }],
        };
        let with_float = RcAstroProcessing {
            steps: vec![RcAstroStep {
                tool: "sxt".into(),
                parameters: BTreeMap::from([(
                    "overlap".to_string(),
                    ExternalParameterValue::Float(1.0),
                )]),
            }],
        };
        let normalized_int = normalize_against_schemas(&with_int, &schemas).unwrap();
        let normalized_float = normalize_against_schemas(&with_float, &schemas).unwrap();
        assert_eq!(normalized_int, normalized_float);
        let id_int = rc_astro_chain_ids("db", "s", "r", &normalized_int, &schemas, None).unwrap();
        let id_float =
            rc_astro_chain_ids("db", "s", "r", &normalized_float, &schemas, None).unwrap();
        assert_eq!(id_int, id_float);

        // Unknown, GUI-only, and out-of-range parameters fail before any
        // tool runs, not minutes into the chain.
        for parameters in [
            BTreeMap::from([("nope".to_string(), ExternalParameterValue::Bool(true))]),
            BTreeMap::from([("csep".to_string(), ExternalParameterValue::Bool(true))]),
            BTreeMap::from([("overlap".to_string(), ExternalParameterValue::Float(9.0))]),
        ] {
            let config = RcAstroProcessing {
                steps: vec![RcAstroStep {
                    tool: "sxt".into(),
                    parameters,
                }],
            };
            assert!(normalize_against_schemas(&config, &schemas).is_err());
        }
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
echo '{{"event":"progress","done":100.0}}'
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
        let mut sxt_schema = schema("sxt", "2.6.6", 11);
        sxt_schema.parameters = vec![serde_json::from_value(serde_json::json!({
            "name": "stars",
            "flag": "--stars",
            "label": "Generate Star Image",
            "description": "",
            "kind": {"type": "bool", "default": false},
        }))
        .unwrap()];
        let schemas = vec![("sxt".to_string(), sxt_schema)];
        let image = LinearImage::new(3, 2, 1, vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0]).unwrap();

        let mut reported: Vec<(String, f32)> = Vec::new();
        let first = apply_rc_astro(
            cache.path(),
            &["a".repeat(64)],
            &config,
            &schemas,
            &image,
            &[],
            &mut |stage, fraction| reported.push((stage.to_string(), fraction)),
        )
        .unwrap();
        assert!(first.result.has_stars);
        assert!(first.stars.is_some());
        for (processed, original) in first.image.data.iter().zip(&image.data) {
            assert!((processed - original).abs() < 1e-2);
        }
        // A one-step chain reports the step start at 0 and the tool's own
        // fractions unscaled.
        assert_eq!(
            reported.first().map(|(stage, _)| stage.as_str()),
            Some("RC-Astro sxt")
        );
        assert_eq!(reported.first().map(|(_, fraction)| *fraction), Some(0.0));
        assert!(reported.iter().any(|(_, fraction)| *fraction >= 1.0));

        let second = apply_rc_astro(
            cache.path(),
            &["a".repeat(64)],
            &config,
            &schemas,
            &image,
            &[],
            &mut |_, _| {},
        )
        .unwrap();
        assert!(second.result.has_stars);

        let invocations =
            std::fs::read_to_string(tool_dir.path().join("invocations")).unwrap_or_default();
        assert_eq!(
            invocations.lines().count(),
            1,
            "second apply must be a cache hit"
        );

        // The regression this cache layout exists for: run BlurX alone, then
        // add NoiseX. The longer chain must resume from the cached BlurX
        // artifact instead of running BlurX again. (Same test because the
        // env-var override is process-wide.)
        let bxt = RcAstroProcessing {
            steps: vec![step("bxt")],
        };
        let bxt_nxt = RcAstroProcessing {
            steps: vec![step("bxt"), step("nxt")],
        };
        let chain_schemas = vec![
            ("bxt".to_string(), schema("bxt", "2.6.6", 4)),
            ("nxt".to_string(), schema("nxt", "2.6.6", 3)),
        ];
        let bxt_ids = vec!["b".repeat(64)];
        let both_ids = vec!["b".repeat(64), "c".repeat(64)];
        apply_rc_astro(
            cache.path(),
            &bxt_ids,
            &bxt,
            &chain_schemas,
            &image,
            &[],
            &mut |_, _| {},
        )
        .unwrap();
        let mut reported: Vec<(String, f32)> = Vec::new();
        let extended = apply_rc_astro(
            cache.path(),
            &both_ids,
            &bxt_nxt,
            &chain_schemas,
            &image,
            &[],
            &mut |stage, fraction| reported.push((stage.to_string(), fraction)),
        )
        .unwrap();
        assert_eq!(extended.result.steps.len(), 2);
        // The resumed chain reports only the step it actually runs, at its
        // place in the whole chain: NoiseX starts at fraction 1/2.
        assert_eq!(
            reported.first().map(|(stage, _)| stage.as_str()),
            Some("RC-Astro nxt")
        );
        assert_eq!(reported.first().map(|(_, fraction)| *fraction), Some(0.5));
        let invocations =
            std::fs::read_to_string(tool_dir.path().join("invocations")).unwrap_or_default();
        assert_eq!(
            invocations.lines().count(),
            3,
            "adding NoiseX must reuse the cached BlurX artifact: expected \
             one sxt run, one bxt run, and one nxt run"
        );

        // The other half of the claim: a longer chain persists its
        // intermediate prefixes as it runs. Run [bxt, nxt] under fresh
        // identities, then ask for [bxt] alone with the prefix id — a full
        // cache hit with no tool spawned.
        let fresh_ids = vec!["e".repeat(64), "f".repeat(64)];
        apply_rc_astro(
            cache.path(),
            &fresh_ids,
            &bxt_nxt,
            &chain_schemas,
            &image,
            &[],
            &mut |_, _| {},
        )
        .unwrap();
        let bxt_alone = apply_rc_astro(
            cache.path(),
            &fresh_ids[..1],
            &bxt,
            &chain_schemas,
            &image,
            &[],
            &mut |_, _| {},
        )
        .unwrap();
        assert_eq!(bxt_alone.result.steps.len(), 1);

        // A full-chain hit must keep the prefixes under it alive: age the
        // BlurX prefix manifest past the sweep, serve the chain from cache,
        // and the prefix manifest must come back fresh.
        let bxt_manifest = rc_astro_manifest_path(cache.path(), &fresh_ids[0]);
        let a_month_ago =
            std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 24 * 3600);
        std::fs::File::options()
            .append(true)
            .open(&bxt_manifest)
            .unwrap()
            .set_modified(a_month_ago)
            .unwrap();
        apply_rc_astro(
            cache.path(),
            &fresh_ids,
            &bxt_nxt,
            &chain_schemas,
            &image,
            &[],
            &mut |_, _| {},
        )
        .unwrap();
        let prefix_age = std::fs::metadata(&bxt_manifest)
            .unwrap()
            .modified()
            .unwrap()
            .elapsed()
            .unwrap_or_default();
        assert!(
            prefix_age < std::time::Duration::from_secs(3600),
            "a full-chain cache hit must refresh the intermediate prefix \
             manifests, or the sweep strands the chain's resume points"
        );
        unsafe { std::env::remove_var("PSF_GUARD_RC_ASTRO") };

        let invocations =
            std::fs::read_to_string(tool_dir.path().join("invocations")).unwrap_or_default();
        assert_eq!(
            invocations.lines().count(),
            5,
            "the fresh [bxt, nxt] run adds two invocations; the [bxt] \
             replay and the full-chain replay must both be cache hits"
        );
    }
}
