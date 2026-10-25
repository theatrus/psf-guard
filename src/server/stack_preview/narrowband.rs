//! Optional automatic narrowband policy and color-star processing. Existing
//! requests carry no policy and retain their original channel-card behavior.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorStars {
    Automatic,
    Rgb,
    Narrowband,
    None,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NarrowbandProcessing {
    #[serde(default)]
    pub shared_stretch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<seiza_stacking::NarrowbandBalanceOptions>,
    /// Optional sharpening/denoising on linear RGB before calibration and SXT.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb_star_processing: Option<super::super::rc_astro::RcAstroProcessing>,
    /// SXT-only settings for each linear branch. Absent keeps the original
    /// managed separation defaults; the star policy controls whether it runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nebula_star_separation: Option<super::super::rc_astro::RcAstroProcessing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb_star_separation: Option<super::super::rc_astro::RcAstroProcessing>,
    /// Independent SetiAstro-style linear star residual stretch [0,8].
    /// None preserves the legacy paired RGB Auto MTF transfer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb_star_stretch: Option<f64>,
    /// Aesthetic green excess reduction on mapped display nebula, before RGB stars.
    #[serde(default)]
    pub green_reduction: f64,
    /// Linked display-star erosion after RGB extraction; zero preserves stars.
    #[serde(default, skip_serializing_if = "stars_unreduced")]
    pub star_reduction: f64,
}

fn stars_unreduced(amount: &f64) -> bool {
    *amount == 0.0
}

impl NarrowbandProcessing {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self
            .rgb_star_stretch
            .is_some_and(|a| !a.is_finite() || !(0.0..=8.0).contains(&a))
        {
            return Err("RGB star stretch amount must be between zero and eight".into());
        }
        if !self.star_reduction.is_finite() || !(0.0..=1.0).contains(&self.star_reduction) {
            return Err("RGB star reduction must be between zero and one".into());
        }
        if !self.green_reduction.is_finite() || !(0.0..=1.0).contains(&self.green_reduction) {
            return Err("Green reduction must be between zero and one".into());
        }
        if let Some(options) = self.balance
            && (options
                .star_mask_radius
                .is_some_and(|r| !r.is_finite() || !(1.0..=64.0).contains(&r))
                || !options.shadow_protection.is_finite()
                || !(0.0..=1.0).contains(&options.shadow_protection)
                || [options.oiii_boost, options.sii_boost]
                    .iter()
                    .any(|v| !v.is_finite() || *v <= 0.0)
                || !(4..=1_000_000).contains(&options.max_samples))
        {
            return Err("Shadow protection must be 0 to 1, boosts positive and finite, and balance samples 4 to 1000000".into());
        }
        for config in [&self.nebula_star_separation, &self.rgb_star_separation]
            .into_iter()
            .flatten()
        {
            validate_separation(config)?;
        }
        if let Some(config) = &self.rgb_star_processing {
            config.validate()?;
            if config
                .steps
                .iter()
                .any(|step| !matches!(step.tool.as_str(), "bxt" | "nxt"))
            {
                return Err("RGB star processing supports BlurXTerminator and NoiseXTerminator before star separation".into());
            }
        }
        Ok(())
    }
}

/// Exact cached linear SXT outputs used by this final composition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinearStarSeparationResult {
    pub cache_id: String,
    pub processing: super::super::rc_astro::RcAstroProcessing,
    #[serde(flatten)]
    pub output: StackColorRcAstroResult,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NarrowbandResolution {
    pub stars: ColorStars,
    pub detail: String,
    pub available_sources: String,
    #[serde(default)]
    pub rc_astro_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_calibration: Option<StackColorCalibrationOutcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb_star_processing: Option<StackColorRcAstroResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nebula_star_separation: Option<LinearStarSeparationResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rgb_star_separation: Option<LinearStarSeparationResult>,
}

pub(super) fn rgb_calibration_settings(job: &StackColorJob) -> Option<StackColorCalibration> {
    job.narrowband_resolution
        .as_ref()
        .filter(|resolution| {
            resolution.stars == ColorStars::Rgb
                || job.combination == Some(emission::CombinedColorRecipe::BroadbandStructure)
        })
        .map(|_| {
            job.processing
                .as_ref()
                .and_then(|p| p.color_calibration)
                .unwrap_or_default()
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StarsPlan {
    pub config: super::super::rc_astro::RcAstroProcessing,
    #[serde(default)]
    pub rgb_config: Option<super::super::rc_astro::RcAstroProcessing>,
    pub schemas: Vec<(String, seiza_stacking::ExternalToolSchema)>,
    pub nebula_chain: Vec<String>,
    pub rgb_chain: Vec<String>,
    pub calibration: Option<ColorCalibrationPlan>,
    #[serde(default)]
    pub rgb_preparation: Option<RgbStarPreparation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct RgbStarPreparation {
    pub config: super::super::rc_astro::RcAstroProcessing,
    pub schemas: Vec<(String, seiza_stacking::ExternalToolSchema)>,
    pub chain: Vec<String>,
}

pub(super) fn available_sources(target: &TargetSources) -> String {
    let mut values: Vec<_> = target
        .by_role
        .iter()
        .flat_map(|(role, sources)| {
            sources.iter().map(move |source| {
                (
                    *role,
                    source.job_id.as_str(),
                    source.group_index,
                    source.artifact_revision.as_str(),
                )
            })
        })
        .collect();
    values.sort();
    Sha256::digest(serde_json::to_vec(&values).expect("source identities serialize"))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Resolve policy before selecting sources. Tool probing here is short-lived;
/// all image work still runs later in the tracked color job.
type SeparationTool = (
    super::super::rc_astro::RcAstroProcessing,
    super::super::rc_astro::RcAstroProcessing,
    Vec<(String, seiza_stacking::ExternalToolSchema)>,
);
type ResolvedSources = (
    Vec<StackColorSource>,
    Option<NarrowbandResolution>,
    Option<SeparationTool>,
);

pub(super) fn resolve(
    workers: &crate::server::workers::Workers,
    target: &TargetSources,
    request: &mut StackColorRequest,
) -> Result<ResolvedSources, AppError> {
    if request.kind != StackColorKind::Narrowband {
        return Ok((select_sources(target, request)?, None, None));
    }
    let combined = request.combination == Some(emission::CombinedColorRecipe::BroadbandStructure);
    let may_fallback = request.stars == Some(ColorStars::Automatic) && !combined;
    for (role, selected) in &request.input_sources {
        if !target.by_role.get(role).is_some_and(|values| {
            values.iter().any(|source| {
                source.job_id == selected.job_id
                    && source.group_index == selected.group_index
                    && source.artifact_revision == selected.artifact_revision
            })
        }) {
            return Err(AppError::BadRequest(format!(
                "The selected {} stack is no longer current; choose it again",
                role.label()
            )));
        }
    }
    if request.automatic_palette {
        let selected: Vec<_> = [StackColorRole::Ha, StackColorRole::Oiii]
            .iter()
            .filter_map(|role| {
                let candidates = target.by_role.get(role)?;
                if let Some(chosen) = request.input_sources.get(role) {
                    candidates.iter().find(|source| {
                        source.job_id == chosen.job_id && source.group_index == chosen.group_index
                    })
                } else {
                    match candidates.as_slice() {
                        [only] => Some(only),
                        _ => None,
                    }
                }
            })
            .collect();
        let sii: Vec<_> = target
            .by_role
            .get(&StackColorRole::Sii)
            .into_iter()
            .flatten()
            .filter(|source| {
                if let Some(chosen) = request.input_sources.get(&StackColorRole::Sii) {
                    return source.job_id == chosen.job_id
                        && source.group_index == chosen.group_index;
                }
                selected.iter().all(|nebula| source.wbpp == nebula.wbpp)
                    && same_exposure_band(&selected, source)
            })
            .collect();
        let has_sii = sii.len() == 1;
        if let [source] = sii.as_slice() {
            request.input_sources.insert(
                StackColorRole::Sii,
                StackColorSourceRef {
                    job_id: source.job_id.clone(),
                    group_index: source.group_index,
                    artifact_revision: source.artifact_revision.clone(),
                },
            );
        }
        request.palette = Some(if has_sii {
            StackNarrowbandPalette::Sho
        } else {
            StackNarrowbandPalette::Hoo
        });
    }
    let mut sources = select_sources(target, request)?;
    let Some(choice) = request.stars else {
        return Ok((
            sources,
            request.automatic_palette.then(|| NarrowbandResolution {
                stars: ColorStars::Narrowband,
                detail: "Star processing follows the channel cards".into(),
                available_sources: available_sources(target),
                rc_astro_ids: Vec::new(),
                color_calibration: None,
                rgb_star_processing: None,
                nebula_star_separation: None,
                rgb_star_separation: None,
            }),
            None,
        ));
    };
    let mut resolution = NarrowbandResolution {
        stars: choice,
        detail: "Stars from the narrowband inputs".into(),
        available_sources: available_sources(target),
        rc_astro_ids: Vec::new(),
        color_calibration: None,
        rgb_star_processing: None,
        nebula_star_separation: None,
        rgb_star_separation: None,
    };
    if choice == ColorStars::Narrowband {
        return Ok((sources, Some(resolution), None));
    }
    let config = super::super::rc_astro::RcAstroProcessing {
        steps: vec![super::super::rc_astro::RcAstroStep {
            tool: "sxt".into(),
            parameters: BTreeMap::from([(
                "stars".into(),
                seiza_stacking::ExternalParameterValue::Bool(true),
            )]),
        }],
    };
    let schemas = match super::super::rc_astro::schemas_for(workers, &config) {
        Ok(schemas) => schemas,
        Err(error) if may_fallback => {
            resolution.stars = ColorStars::Narrowband;
            resolution.detail = format!(
                "Automatic uses narrowband stars: star separation is unavailable ({error})"
            );
            return Ok((sources, Some(resolution), None));
        }
        Err(error) => {
            return Err(AppError::BadRequest(format!(
                "The selected star source requires StarXTerminator: {error}"
            )))
        }
    };
    let settings = request
        .processing
        .as_ref()
        .and_then(|p| p.narrowband.as_ref());
    let requested_nebula = settings.and_then(|p| p.nebula_star_separation.as_ref());
    let config = match separation_config(requested_nebula, &schemas) {
        Ok(config) => config,
        Err(error) if may_fallback && requested_nebula.is_none() => {
            resolution.stars = ColorStars::Narrowband;
            resolution.detail = format!(
                "Automatic uses narrowband stars: star separation is unavailable ({error})"
            );
            return Ok((sources, Some(resolution), None));
        }
        Err(error) => return Err(AppError::BadRequest(error)),
    };
    let rgb_config = if choice != ColorStars::None || combined {
        separation_config(
            settings.and_then(|p| p.rgb_star_separation.as_ref()),
            &schemas,
        )
        .map_err(AppError::BadRequest)?
    } else {
        separation_config(None, &schemas).map_err(AppError::BadRequest)?
    };
    if choice != ColorStars::None || combined {
        let rgb_request = StackColorRequest {
            kind: StackColorKind::Rgb,
            combination: None,
            palette: None,
            automatic_palette: false,
            stars: None,
            processing: None,
            ..request.clone()
        };
        let rgb_target = TargetSources {
            target_name: target.target_name.clone(),
            unmapped_filters: Vec::new(),
            by_role: target
                .by_role
                .iter()
                .map(|(role, candidates)| {
                    let keep_origin = request.input_sources.contains_key(role);
                    (
                        *role,
                        candidates
                            .iter()
                            .filter(|candidate| {
                                keep_origin
                                    || sources.iter().any(|nebula| nebula.wbpp == candidate.wbpp)
                            })
                            .cloned()
                            .collect(),
                    )
                })
                .collect(),
        };
        match select_sources(&rgb_target, &rgb_request) {
            Ok(rgb) => {
                for source in rgb {
                    if !sources.iter().any(|old| old.role == source.role) {
                        sources.push(source);
                    }
                }
            }
            Err(error) if may_fallback => {
                resolution.stars = ColorStars::Narrowband;
                let error = match error {
                    AppError::BadRequest(message) => message,
                    _ => "RGB sources are unavailable".into(),
                };
                resolution.detail = format!(
                    "Automatic uses narrowband stars: RGB sources need attention ({error})"
                );
                return Ok((sources, Some(resolution), None));
            }
            Err(error) => return Err(error),
        }
        if choice != ColorStars::None {
            resolution.stars = ColorStars::Rgb;
            resolution.detail = "RGB stars replace narrowband stars on the starless nebula".into();
        } else {
            resolution.detail = "Starless narrowband and broadband structures".into();
        }
    } else {
        resolution.detail = "Starless narrowband nebula".into();
    }
    Ok((
        sources,
        Some(resolution),
        Some((config, rgb_config, schemas)),
    ))
}

fn validate_separation(config: &super::super::rc_astro::RcAstroProcessing) -> Result<(), String> {
    config.validate()?;
    if config.steps.len() != 1 || config.steps[0].tool != "sxt" {
        return Err("Managed star separation supports only StarXTerminator".into());
    }
    for (name, value) in &config.steps[0].parameters {
        if ((name == "stars" || name == "difference")
            && *value != seiza_stacking::ExternalParameterValue::Bool(true))
            || (name == "unscreen" && *value != seiza_stacking::ExternalParameterValue::Bool(false))
        {
            return Err(
                "Linear star separation must keep the stars and leave Unscreen Stars off".into(),
            );
        }
    }
    Ok(())
}

/// Keep tool output semantics fixed while allowing the live SXT parameters.
/// Canonical defaults retain the existing managed-run cache identities.
fn separation_config(
    requested: Option<&super::super::rc_astro::RcAstroProcessing>,
    schemas: &[(String, seiza_stacking::ExternalToolSchema)],
) -> Result<super::super::rc_astro::RcAstroProcessing, String> {
    use super::super::rc_astro::{RcAstroProcessing, RcAstroStep};
    let schema = &schemas
        .first()
        .ok_or("StarXTerminator schema is unavailable")?
        .1;
    let mut config = requested.cloned().unwrap_or(RcAstroProcessing {
        steps: vec![RcAstroStep {
            tool: "sxt".into(),
            parameters: BTreeMap::new(),
        }],
    });
    validate_separation(&config)?;
    let star_parameter = schema
        .parameters
        .iter()
        .find(|p| matches!(p.name.as_str(), "stars" | "difference") && p.flag.is_some())
        .ok_or("StarXTerminator cannot save its linear stars output")?;
    let parameters = &mut config.steps[0].parameters;
    parameters.retain(|name, _| name != "stars" && name != "difference");
    parameters.insert(
        star_parameter.name.clone(),
        seiza_stacking::ExternalParameterValue::Bool(true),
    );
    if let Some(unscreen) = schema
        .parameters
        .iter()
        .find(|p| p.name == "unscreen" && p.flag.is_some())
    {
        match unscreen.kind {
            seiza_stacking::ExternalParameterKind::Bool { default: false } => {
                parameters.remove("unscreen");
            }
            _ => {
                parameters.insert(
                    "unscreen".into(),
                    seiza_stacking::ExternalParameterValue::Bool(false),
                );
            }
        }
    }
    let mut config = super::super::rc_astro::normalize_against_schemas(&config, schemas)?;
    config.steps[0].parameters.retain(|name, value| {
        if name == &star_parameter.name {
            return true;
        }
        !schema.parameters.iter().any(|p| {
            p.name == *name
                && match (&p.kind, &*value) {
                    (
                        seiza_stacking::ExternalParameterKind::Bool { default },
                        seiza_stacking::ExternalParameterValue::Bool(v),
                    ) => default == v,
                    (
                        seiza_stacking::ExternalParameterKind::Float { default, .. },
                        seiza_stacking::ExternalParameterValue::Float(v),
                    ) => default == v,
                    (
                        seiza_stacking::ExternalParameterKind::Int { default, .. },
                        seiza_stacking::ExternalParameterValue::Int(v),
                    ) => default == v,
                    _ => false,
                }
        })
    });
    Ok(config)
}

/// A final composition keeps links to the exact linear results. If either
/// half disappears, rebuilding repairs that run instead of retaining dead links.
pub(super) fn separation_artifacts_exist(
    root: &FsPath,
    resolution: Option<&NarrowbandResolution>,
) -> bool {
    resolution
        .into_iter()
        .flat_map(|r| [&r.nebula_star_separation, &r.rgb_star_separation])
        .flatten()
        .all(|report| {
            super::super::validate_job_id(&report.cache_id).is_ok()
                && super::super::rc_astro::rc_astro_chain_cached(
                    root,
                    std::slice::from_ref(&report.cache_id),
                    &report.processing,
                )
        })
}

fn separation_result(
    database_id: &str,
    chain: &[String],
    config: &super::super::rc_astro::RcAstroProcessing,
    result: super::super::rc_astro::StackRcAstroResult,
) -> Result<LinearStarSeparationResult, String> {
    let id = chain.last().ok_or("Star separation chain is empty")?;
    let fits_url = format!("/api/db/{database_id}/stack-previews/rc-astro/{id}/fits");
    Ok(LinearStarSeparationResult {
        cache_id: id.clone(),
        processing: config.clone(),
        output: StackColorRcAstroResult {
            stars_fits_url: result.has_stars.then(|| format!("{fits_url}?stars=true")),
            fits_url,
            result,
        },
    })
}

fn same_exposure_band(selected: &[&StackColorSource], candidate: &StackColorSource) -> bool {
    if selected.is_empty() {
        return true;
    }
    let sources = selected.iter().copied().chain(std::iter::once(candidate));
    if sources
        .clone()
        .all(|source| source.exposure_group.is_none())
    {
        return true;
    }
    let mut low = f64::INFINITY;
    let mut high = 0.0_f64;
    for source in sources {
        let Some(group) = &source.exposure_group else {
            return false;
        };
        let (Some(min), Some(max)) = (group.min_seconds, group.max_seconds) else {
            return false;
        };
        if !min.is_finite() || !max.is_finite() || min <= 0.0 || max < min {
            return false;
        }
        low = low.min(min);
        high = high.max(max);
    }
    high < 2.0 * low
}

pub(super) fn defer_channel_star_removal(processing: &mut StackColorProcessing) {
    for config in processing.input_rc_astro.values_mut() {
        config.steps.retain(|step| step.tool != "sxt");
    }
    processing
        .input_rc_astro
        .retain(|_, config| !config.steps.is_empty());
    if let Some(config) = &mut processing.color_rc_astro {
        config.steps.retain(|step| step.tool != "sxt");
        if config.steps.is_empty() {
            processing.color_rc_astro = None;
        }
    }
}

pub(super) struct RgbStarsDisplay {
    pub layer: Option<LinearImage>,
    pub original: LinearImage,
    pub starless: LinearImage,
}

pub(super) fn combine(
    images: &BTreeMap<StackColorRole, LinearImage>,
    roles: [StackColorRole; 3],
) -> Result<LinearImage, String> {
    let planes = roles.map(|role| {
        images
            .get(&role)
            .ok_or_else(|| format!("{} input is missing", role.label()))
    });
    let [red, green, blue] = planes;
    combine_rgb(
        red?,
        green?,
        blue?,
        &ColorOptions {
            normalization: ColorNormalization::None,
            ..Default::default()
        },
    )
    .map(|value| value.image)
    .map_err(|error| error.to_string())
}

pub(super) fn put_planes(
    images: &mut BTreeMap<StackColorRole, LinearImage>,
    combined: LinearImage,
    roles: [StackColorRole; 3],
) -> Result<(), String> {
    for (channel, role) in roles.iter().enumerate() {
        if roles[..channel].contains(role) {
            continue;
        }
        images.insert(
            *role,
            LinearImage::new(
                combined.width,
                combined.height,
                1,
                combined
                    .data
                    .iter()
                    .skip(channel)
                    .step_by(3)
                    .copied()
                    .collect(),
            )
            .map_err(|error| error.to_string())?,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn separate_stars(
    state: &Arc<AppState>,
    job: &StackColorJob,
    plan: &StarsPlan,
    root: &FsPath,
    images: &mut BTreeMap<StackColorRole, LinearImage>,
    headers: &[(String, seiza_fits::HeaderValue)],
    progress: &ColorProgressTracker<'_>,
    stop: &ColorStop,
    open_catalog: OpenCatalog<'_>,
) -> Result<Option<RgbStarsDisplay>, String> {
    let phase = StackColorProgressPhase::SeparatingStars;
    progress.begin(
        phase,
        "Preparing narrowband nebula and RGB stars",
        None,
        None,
    );
    let runner = crate::server::workers::rc_astro::RcAstroRunner {
        workers: &state.workers,
        person: !job.automatic,
    };
    let run = |chain: &[String],
               config: &super::super::rc_astro::RcAstroProcessing,
               image: &LinearImage,
               label: &str| {
        super::super::rc_astro::apply_rc_astro(
            runner,
            root,
            chain,
            config,
            &plan.schemas,
            image,
            headers,
            &mut |_, fraction| {
                progress.begin(
                    phase,
                    format!("{label}: StarXTerminator ({:.0}%)", fraction * 100.0),
                    None,
                    None,
                );
                progress.partial(1, fraction);
            },
            Some(&stop.signal()),
            &mut || stop.check(),
        )
    };
    let result = (|| {
        let mut rgb_result = None;
        let rgb = if let Some(calibration) = &plan.calibration {
            if let Some(preparation) = &plan.rgb_preparation {
                let combined = combine(images, RGB_ROLES)?;
                let prepared = super::super::rc_astro::apply_rc_astro(
                    runner,
                    root,
                    &preparation.chain,
                    &preparation.config,
                    &preparation.schemas,
                    &combined,
                    headers,
                    &mut |label, fraction| {
                        progress.begin(
                            phase,
                            format!("RGB star processing: {label} ({:.0}%)", fraction * 100.0),
                            None,
                            None,
                        );
                        progress.partial(1, fraction);
                    },
                    Some(&stop.signal()),
                    &mut || stop.check(),
                )?;
                let id = preparation
                    .chain
                    .last()
                    .ok_or("RGB star processing chain is empty")?;
                let report = StackColorRcAstroResult {
                    result: prepared.result,
                    fits_url: format!(
                        "/api/db/{}/stack-previews/rc-astro/{id}/fits",
                        job.database_id
                    ),
                    stars_fits_url: None,
                };
                put_planes(images, prepared.image, RGB_ROLES)?;
                state.stack_previews.update_color(&job.job_id, |current| {
                    if let Some(resolution) = &mut current.narrowband_resolution {
                        resolution.rgb_star_processing = Some(report);
                    }
                });
                progress.advance(phase, preparation.config.steps.len());
                stop.check()?;
            }
            let outcome = calibration
                .settings
                .enabled
                .then(|| match &calibration.placement {
                    Ok(placement) => calibrate_registered_channels(
                        placement,
                        &calibration.settings,
                        images,
                        None,
                        open_catalog,
                    ),
                    Err(reason) => StackColorCalibrationOutcome::Skipped(reason.clone()),
                });
            state.stack_previews.update_color(&job.job_id, |current| {
                if let Some(resolution) = &mut current.narrowband_resolution {
                    resolution.color_calibration = outcome;
                }
            });
            let original = combine(images, RGB_ROLES)?;
            let config = plan.rgb_config.as_ref().unwrap_or(&plan.config);
            let output = run(&plan.rgb_chain, config, &original, "RGB stars")?;
            rgb_result = Some(separation_result(
                &job.database_id,
                &plan.rgb_chain,
                config,
                output.result.clone(),
            )?);
            stop.check()?;
            // SXT has run on calibrated LINEAR RGB. Extract and stretch the
            // linear residual before any display transfer. The separate
            // starless RGB display below still supplies broadband structure.
            let layer = job
                .processing
                .as_ref()
                .and_then(|p| p.narrowband.as_ref())
                .and_then(|p| p.rgb_star_stretch)
                .map(|amount| seiza_stacking::stretch_rgb_stars(&original, &output.image, amount))
                .transpose()
                .map_err(|e| e.to_string())?;
            // Resolve once from star-containing calibrated RGB, then apply
            // exactly that same display transfer to its starless counterpart.
            let curve = super::super::stretch::default_linear_config()
                .resolve_for(&original.data, 3)
                .map_err(|error| error.to_string())?;
            let shape = super::super::stretch::image_shape(&original);
            let mut display_original = curve
                .apply_f32_image(&original.data, shape)
                .map_err(|error| error.to_string())?;
            let mut display_starless = curve
                .apply_f32_image(&output.image.data, shape)
                .map_err(|error| error.to_string())?;
            for ((original_pixel, starless_pixel), (input, separated)) in display_original
                .chunks_mut(3)
                .zip(display_starless.chunks_mut(3))
                .zip(original.data.chunks(3).zip(output.image.data.chunks(3)))
            {
                if input.iter().chain(separated).any(|v| !v.is_finite()) {
                    original_pixel.fill(f32::NAN);
                    starless_pixel.fill(f32::NAN);
                }
            }
            Some(RgbStarsDisplay {
                layer,
                original: LinearImage::new(original.width, original.height, 3, display_original)
                    .map_err(|error| error.to_string())?,
                starless: LinearImage::new(original.width, original.height, 3, display_starless)
                    .map_err(|error| error.to_string())?,
            })
        } else {
            None
        };
        let roles = color_channel_roles(job.kind, job.palette);
        let physical = combine(images, roles)?;
        let separated = run(
            &plan.nebula_chain,
            &plan.config,
            &physical,
            "Narrowband nebula",
        )?;
        let nebula_result = separation_result(
            &job.database_id,
            &plan.nebula_chain,
            &plan.config,
            separated.result,
        )?;
        let mut output = separated.image;
        stop.check()?;
        if let Some(rgb) = &rgb {
            // The output crop must also respect the RGB footprint. Do this
            // before composition, so inscribed cropping sees that footprint.
            for ((nebula, original), starless) in output
                .data
                .chunks_mut(3)
                .zip(rgb.original.data.chunks(3))
                .zip(rgb.starless.data.chunks(3))
            {
                if original.iter().chain(starless).any(|v| !v.is_finite()) {
                    nebula.fill(f32::NAN);
                }
            }
        }
        put_planes(images, output, roles)?;
        state.stack_previews.update_color(&job.job_id, |current| {
            if let Some(resolution) = &mut current.narrowband_resolution {
                resolution.nebula_star_separation = Some(nebula_result);
                resolution.rgb_star_separation = rgb_result;
            }
        });
        Ok::<_, String>(rgb)
    })();
    match result {
        Ok(rgb) => {
            progress.advance(phase, 2);
            progress.finish(phase);
            Ok(rgb)
        }
        Err(error) => {
            stop.check()?;
            if job.stars != Some(ColorStars::Automatic)
                || job.combination == Some(emission::CombinedColorRecipe::BroadbandStructure)
            {
                return Err(error);
            }
            // The nebula planes are committed only after both separations
            // succeed, so this fallback keeps their original stars.
            let detail =
                format!("Automatic uses narrowband stars: star separation failed ({error})");
            state.stack_previews.update_color(&job.job_id, |current| {
                if let Some(resolution) = &mut current.narrowband_resolution {
                    resolution.stars = ColorStars::Narrowband;
                    resolution.detail = detail.clone();
                    resolution.nebula_star_separation = None;
                    resolution.rgb_star_separation = None;
                }
            });
            progress.skip(phase, detail);
            Ok(None)
        }
    }
}

pub(super) fn shared_stretch(
    state: &Arc<AppState>,
    job: &StackColorJob,
    processing: &StackColorProcessing,
    images: &mut BTreeMap<StackColorRole, LinearImage>,
    progress: &ColorProgressTracker<'_>,
    stop: &ColorStop,
) -> Result<(), String> {
    let phase = StackColorProgressPhase::StretchingInputs;
    progress.begin(
        phase,
        "Applying one shared H-alpha stretch to the narrowband channels",
        None,
        None,
    );
    let roles = composed_roles(job.kind, job.palette);
    let stages = processing
        .input_stretches
        .get(&StackColorRole::Ha)
        .cloned()
        .unwrap_or_default();
    let mut plans = Vec::new();
    for stage in stages {
        stop.check()?;
        let reference = &images[&StackColorRole::Ha];
        let plan = stage
            .config()
            .resolve_for(&reference.data, 1)
            .map_err(|error| error.to_string())?;
        for role in &roles {
            let image = images
                .get_mut(role)
                .ok_or_else(|| format!("{} input is missing", role.label()))?;
            let mut stretched = plan
                .apply_f32_image(&image.data, super::super::stretch::image_shape(image))
                .map_err(|error| error.to_string())?;
            for (output, input) in stretched.iter_mut().zip(&image.data) {
                if !input.is_finite() {
                    *output = f32::NAN;
                }
            }
            image.data = stretched;
        }
        plans.push(serde_json::to_value(plan).map_err(|error| error.to_string())?);
        progress.advance(phase, roles.len());
    }
    state.stack_previews.update_color(&job.job_id, |current| {
        for role in &roles {
            current
                .resolved_input_stretches
                .insert(*role, plans.clone());
        }
    });
    progress.finish(phase);
    Ok(())
}

pub(super) fn balance(
    state: &Arc<AppState>,
    job: &StackColorJob,
    options: &seiza_stacking::NarrowbandBalanceOptions,
    images: &mut BTreeMap<StackColorRole, LinearImage>,
    progress: &ColorProgressTracker<'_>,
) -> Result<(), String> {
    let phase = StackColorProgressPhase::BalancingNarrowband;
    progress.begin(
        phase,
        "Balancing physical narrowband channels to H-alpha",
        None,
        None,
    );
    let roles = color_channel_roles(job.kind, job.palette);
    let mut physical = combine(images, roles)?;
    let options = seiza_stacking::NarrowbandBalanceOptions {
        has_sii: roles[2] == StackColorRole::Sii,
        ..*options
    };
    let report = seiza_stacking::balance_narrowband(&mut physical, &options)
        .map_err(|error| error.to_string())?;
    put_planes(images, physical, roles)?;
    state.stack_previews.update_color(&job.job_id, |current| {
        current.narrowband_balance = Some(report);
    });
    progress.advance(phase, 1);
    progress.finish(phase);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::workers::Workers;

    fn source(role: StackColorRole) -> StackColorSource {
        serde_json::from_value(serde_json::json!({
            "role": role, "filter_name": role.label(), "job_id": format!("{role:?}"),
            "group_index": 0, "artifact_revision": "first", "accepted_frames": 5,
        }))
        .unwrap()
    }
    fn target(rgb: bool, sii: bool) -> TargetSources {
        let mut roles = vec![StackColorRole::Ha, StackColorRole::Oiii];
        if sii {
            roles.push(StackColorRole::Sii);
        }
        if rgb {
            roles.extend(RGB_ROLES);
        }
        TargetSources {
            target_name: "Nebula".into(),
            unmapped_filters: Vec::new(),
            by_role: roles
                .into_iter()
                .map(|role| (role, vec![source(role)]))
                .collect(),
        }
    }
    fn request(stars: Option<ColorStars>) -> StackColorRequest {
        serde_json::from_value(serde_json::json!({
            "target_id": 42, "kind": "narrowband", "palette": "sho", "automatic_palette": true, "stars": stars,
        })).unwrap()
    }

    #[test]
    fn rgb_stars_check_the_calibration_basis_even_when_the_nebula_does_not_calibrate() {
        let mut job =
            super::super::tests::running_color_job("calibration", StackJobState::Completed);
        job.kind = StackColorKind::Narrowband;
        job.processing = Some(StackColorProcessing::default());
        let mut resolution = NarrowbandResolution {
            stars: ColorStars::Rgb,
            detail: String::new(),
            available_sources: String::new(),
            rc_astro_ids: vec![],
            color_calibration: None,
            rgb_star_processing: None,
            nebula_star_separation: None,
            rgb_star_separation: None,
        };
        for stars in [ColorStars::Rgb, ColorStars::Narrowband, ColorStars::None] {
            resolution.stars = stars;
            job.narrowband_resolution = Some(resolution.clone());
            assert_eq!(
                rgb_calibration_settings(&job).is_some(),
                stars == ColorStars::Rgb
            );
        }
    }

    #[test]
    fn new_star_choices_defer_star_removal_but_keep_the_tool_prefix() {
        use super::super::super::rc_astro::{RcAstroProcessing, RcAstroStep};
        let config = RcAstroProcessing {
            steps: ["bxt", "nxt", "sxt"]
                .map(|tool| RcAstroStep {
                    tool: tool.into(),
                    parameters: BTreeMap::new(),
                })
                .to_vec(),
        };
        let mut processing = StackColorProcessing {
            input_rc_astro: BTreeMap::from([(StackColorRole::Ha, config.clone())]),
            color_rc_astro: Some(config),
            ..Default::default()
        };
        defer_channel_star_removal(&mut processing);
        assert_eq!(
            processing.input_rc_astro[&StackColorRole::Ha]
                .steps
                .iter()
                .map(|step| step.tool.as_str())
                .collect::<Vec<_>>(),
            ["bxt", "nxt"]
        );
        assert_eq!(processing.color_rc_astro.unwrap().steps.len(), 2);
    }

    #[test]
    fn star_stretch_validates_and_only_changes_final_identity() {
        let mut request = request(None);
        let sources = vec![source(StackColorRole::Ha), source(StackColorRole::Oiii)];
        let mut ids = std::collections::HashSet::new();
        for amount in [None, Some(0.0), Some(3.0), Some(5.0)] {
            let config = NarrowbandProcessing {
                rgb_star_stretch: amount,
                ..Default::default()
            };
            config.validate().unwrap();
            request.processing = Some(StackColorProcessing {
                narrowband: Some(config),
                ..Default::default()
            });
            ids.insert(
                color_job_id(&ColorJobIdentity {
                    database_id: "db",
                    project_id: 1,
                    request: &request,
                    label: "SHO",
                    channel_tools_input_id: None,
                    color_rc_astro_id: None,
                    background_protection: &BTreeMap::new(),
                    color_calibration_basis: None,
                    sources: &sources,
                })
                .unwrap(),
            );
        }
        assert_eq!(ids.len(), 4);
        let legacy: NarrowbandProcessing = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(legacy.rgb_star_stretch.is_none());
        assert!(serde_json::to_value(legacy)
            .unwrap()
            .get("rgb_star_stretch")
            .is_none());
        for amount in [f64::NAN, f64::INFINITY, -0.1, 8.1] {
            assert!(NarrowbandProcessing {
                rgb_star_stretch: Some(amount),
                ..Default::default()
            }
            .validate()
            .is_err());
        }
    }

    #[test]
    fn each_star_policy_and_automatic_palette_has_its_own_cache_identity() {
        let mut request = request(None);
        let sources = vec![source(StackColorRole::Ha), source(StackColorRole::Oiii)];
        let ids: std::collections::HashSet<_> = [
            None,
            Some(ColorStars::Automatic),
            Some(ColorStars::Rgb),
            Some(ColorStars::Narrowband),
            Some(ColorStars::None),
        ]
        .into_iter()
        .map(|choice| {
            request.stars = choice;
            color_job_id(&ColorJobIdentity {
                database_id: "db",
                project_id: 1,
                request: &request,
                label: "SHO",
                channel_tools_input_id: None,
                color_rc_astro_id: None,
                background_protection: &BTreeMap::new(),
                color_calibration_basis: None,
                sources: &sources,
            })
            .unwrap()
        })
        .collect();
        assert_eq!(ids.len(), 5);
    }

    #[test]
    fn implicit_narrowband_sources_still_constrain_the_automatic_sii_choice() {
        let mut target = target(false, true);
        let mut request = request(None);
        for (role, sources) in &mut target.by_role {
            let seconds = if *role == StackColorRole::Sii {
                30.0
            } else {
                300.0
            };
            sources[0].exposure_group = Some(crate::server::exposure_groups::ExposureGroup {
                key: seconds.to_string(),
                label: format!("{seconds} s"),
                min_seconds: Some(seconds),
                max_seconds: Some(seconds),
            });
        }
        resolve(&Workers::default(), &target, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Hoo));
        target.by_role.get_mut(&StackColorRole::Sii).unwrap()[0].exposure_group = target.by_role
            [&StackColorRole::Ha][0]
            .exposure_group
            .clone();
        target.by_role.get_mut(&StackColorRole::Sii).unwrap()[0].wbpp = true;
        resolve(&Workers::default(), &target, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Hoo));
    }

    #[test]
    fn automatic_palette_does_not_borrow_sii_from_a_different_exposure_band() {
        let mut target = target(false, true);
        let mut request = request(None);
        for (role, sources) in &mut target.by_role {
            let seconds = if *role == StackColorRole::Sii {
                30.0
            } else {
                300.0
            };
            sources[0].exposure_group = Some(crate::server::exposure_groups::ExposureGroup {
                key: seconds.to_string(),
                label: format!("{seconds} s"),
                min_seconds: Some(seconds),
                max_seconds: Some(seconds),
            });
            if *role != StackColorRole::Sii {
                request.input_sources.insert(
                    *role,
                    StackColorSourceRef {
                        job_id: sources[0].job_id.clone(),
                        group_index: 0,
                        artifact_revision: "first".into(),
                    },
                );
            }
        }
        let (sources, _, _) = resolve(&Workers::default(), &target, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Hoo));
        assert_eq!(sources.len(), 2);
        let mut matching = target.by_role[&StackColorRole::Sii][0].clone();
        matching.job_id = "sii-long".into();
        matching.exposure_group = target.by_role[&StackColorRole::Ha][0]
            .exposure_group
            .clone();
        target
            .by_role
            .get_mut(&StackColorRole::Sii)
            .unwrap()
            .push(matching);
        let (sources, _, _) = resolve(&Workers::default(), &target, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Sho));
        assert!(sources.iter().any(|source| source.job_id == "sii-long"));
    }

    #[test]
    fn automatic_palette_uses_only_unambiguous_physical_filters() {
        let mut request = request(None);
        let target = target(false, true);
        resolve(&Workers::default(), &target, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Sho));
        request.input_sources.clear();
        let mut ambiguous = target;
        let mut second = source(StackColorRole::Sii);
        second.job_id = "another SII".into();
        ambiguous
            .by_role
            .get_mut(&StackColorRole::Sii)
            .unwrap()
            .push(second);
        resolve(&Workers::default(), &ambiguous, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Hoo));
        request.input_sources.insert(
            StackColorRole::Sii,
            StackColorSourceRef {
                job_id: "Sii".into(),
                group_index: 0,
                artifact_revision: "first".into(),
            },
        );
        resolve(&Workers::default(), &ambiguous, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Sho));
        request
            .input_sources
            .get_mut(&StackColorRole::Sii)
            .unwrap()
            .artifact_revision = "stale".into();
        assert!(resolve(&Workers::default(), &ambiguous, &mut request).is_err());
    }

    #[test]
    fn legacy_requests_keep_their_serialized_identity() {
        let request: StackColorRequest = serde_json::from_value(
            serde_json::json!({"target_id": 42, "kind": "narrowband", "palette": "sho"}),
        )
        .unwrap();
        let value = serde_json::to_value(&request).unwrap();
        assert!(value.get("stars").is_none());
        assert!(value.get("automatic_palette").is_none());
        let processing = serde_json::to_value(StackColorProcessing::default()).unwrap();
        assert!(processing.get("narrowband").is_none());
    }

    #[test]
    fn availability_identity_does_not_depend_on_candidate_order() {
        let mut target = target(true, true);
        let mut other = source(StackColorRole::Red);
        other.job_id = "another red".into();
        target
            .by_role
            .get_mut(&StackColorRole::Red)
            .unwrap()
            .push(other);
        let before = available_sources(&target);
        target
            .by_role
            .get_mut(&StackColorRole::Red)
            .unwrap()
            .reverse();
        assert_eq!(before, available_sources(&target));
        target.by_role.get_mut(&StackColorRole::Red).unwrap()[0].artifact_revision = "new".into();
        assert_ne!(before, available_sources(&target));
    }

    #[cfg(unix)]
    #[test]
    fn automatic_stars_uses_rgb_and_reports_incomplete_sources() {
        let _tool = super::super::super::rc_astro::test_tool::install(false);
        let (sources, result, plan) = resolve(
            &Workers::default(),
            &target(true, true),
            &mut request(Some(ColorStars::Automatic)),
        )
        .unwrap();
        assert_eq!(sources.len(), 6);
        assert_eq!(result.unwrap().stars, ColorStars::Rgb);
        assert!(plan.is_some());
        let (sources, result, plan) = resolve(
            &Workers::default(),
            &target(false, true),
            &mut request(Some(ColorStars::Automatic)),
        )
        .unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(result.unwrap().stars, ColorStars::Narrowband);
        assert!(plan.is_none());
        assert!(resolve(
            &Workers::default(),
            &target(false, true),
            &mut request(Some(ColorStars::Rgb))
        )
        .is_err());
        let (_, result, plan) = resolve(
            &Workers::default(),
            &target(false, true),
            &mut request(Some(ColorStars::None)),
        )
        .unwrap();
        assert_eq!(result.unwrap().stars, ColorStars::None);
        assert!(plan.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn rgb_sources_follow_the_nebula_origin_unless_explicitly_selected() {
        let _tool = super::super::super::rc_astro::test_tool::install(false);
        let mut target = target(true, true);
        for (role, sources) in &mut target.by_role {
            if RGB_ROLES.contains(role) {
                let mut wbpp = sources[0].clone();
                wbpp.wbpp = true;
                wbpp.job_id = format!("wbpp-{role:?}");
                sources.push(wbpp);
            } else {
                sources[0].wbpp = true;
            }
        }
        let (selected, _, _) = resolve(
            &Workers::default(),
            &target,
            &mut request(Some(ColorStars::Automatic)),
        )
        .unwrap();
        assert!(selected.iter().all(|source| source.wbpp));
        let mut request = request(Some(ColorStars::Rgb));
        request.input_sources.insert(
            StackColorRole::Red,
            StackColorSourceRef {
                job_id: "Red".into(),
                group_index: 0,
                artifact_revision: "first".into(),
            },
        );
        let (selected, _, _) = resolve(&Workers::default(), &target, &mut request).unwrap();
        assert!(
            !selected
                .iter()
                .find(|source| source.role == StackColorRole::Red)
                .unwrap()
                .wbpp
        );
    }

    #[cfg(unix)]
    #[test]
    fn separation_options_are_independent_and_defaults_reuse_existing_keys() {
        use super::super::super::rc_astro::{rc_astro_chain_ids, RcAstroProcessing};
        let _tool = super::super::super::rc_astro::test_tool::install(false);
        let (_, _, resolved) = resolve(
            &Workers::default(),
            &target(true, true),
            &mut request(Some(ColorStars::Rgb)),
        )
        .unwrap();
        let (nebula, rgb, schemas) = resolved.unwrap();
        assert_eq!(nebula, rgb);
        let default_key =
            rc_astro_chain_ids("db", "linear", "first", &nebula, &schemas, None).unwrap();
        let explicit: RcAstroProcessing = serde_json::from_value(serde_json::json!({"steps":[{"tool":"sxt","parameters":{"overlap":0.2,"stars":true,"unscreen":false}}]})).unwrap();
        assert_eq!(
            separation_config(Some(&explicit), &schemas).unwrap(),
            nebula
        );
        for parameters in [
            serde_json::json!({"unscreen":true}),
            serde_json::json!({"stars":false}),
            serde_json::json!({"overlap":0.8}),
        ] {
            let config = serde_json::from_value(
                serde_json::json!({"steps":[{"tool":"sxt","parameters":parameters}]}),
            )
            .unwrap();
            assert!(separation_config(Some(&config), &schemas).is_err());
        }
        let mut changed = explicit.clone();
        changed.steps[0].parameters.insert(
            "overlap".into(),
            seiza_stacking::ExternalParameterValue::Float(0.3),
        );
        let mut request = request(Some(ColorStars::Rgb));
        request.processing = Some(StackColorProcessing {
            narrowband: Some(NarrowbandProcessing {
                nebula_star_separation: Some(changed),
                ..Default::default()
            }),
            ..Default::default()
        });
        let (_, _, resolved) =
            resolve(&Workers::default(), &target(true, true), &mut request).unwrap();
        let (nebula, rgb, _) = resolved.unwrap();
        assert_ne!(
            rc_astro_chain_ids("db", "linear", "first", &nebula, &schemas, None).unwrap(),
            default_key
        );
        assert_eq!(
            rc_astro_chain_ids("db", "linear", "first", &rgb, &schemas, None).unwrap(),
            default_key
        );
    }

    #[cfg(unix)]
    #[test]
    fn star_separation_reuses_both_caches_and_carries_rgb_coverage_into_the_crop() {
        let tool = super::super::super::rc_astro::test_tool::install(false);
        let (sources, resolution, config) = resolve(
            &Workers::default(),
            &target(true, true),
            &mut request(Some(ColorStars::Rgb)),
        )
        .unwrap();
        let (config, rgb_config, schemas) = config.unwrap();
        let root = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        let mut job =
            super::super::tests::running_color_job(&"a".repeat(64), StackJobState::Running);
        job.kind = StackColorKind::Narrowband;
        job.palette = Some(StackNarrowbandPalette::Sho);
        job.stars = Some(ColorStars::Rgb);
        job.sources = sources;
        job.narrowband_resolution = resolution;
        let preparation_config = super::super::super::rc_astro::RcAstroProcessing {
            steps: vec![super::super::super::rc_astro::RcAstroStep {
                tool: "bxt".into(),
                parameters: Default::default(),
            }],
        };
        let preparation_schemas =
            super::super::super::rc_astro::schemas_for(&Workers::default(), &preparation_config)
                .unwrap();
        let preparation_chain = super::super::super::rc_astro::rc_astro_chain_ids(
            "db",
            "rgb-prepare",
            "first",
            &preparation_config,
            &preparation_schemas,
            None,
        )
        .unwrap();
        let plan = StarsPlan {
            rgb_config: Some(rgb_config),
            rgb_preparation: Some(RgbStarPreparation {
                config: preparation_config,
                schemas: preparation_schemas,
                chain: preparation_chain,
            }),
            nebula_chain: super::super::super::rc_astro::rc_astro_chain_ids(
                "db", "nebula", "first", &config, &schemas, None,
            )
            .unwrap(),
            rgb_chain: super::super::super::rc_astro::rc_astro_chain_ids(
                "db", "rgb", "first", &config, &schemas, None,
            )
            .unwrap(),
            config,
            schemas,
            calibration: Some(color_calibration::plan(
                Default::default(),
                None,
                None,
                None,
                None,
            )),
        };
        job.narrowband_resolution.as_mut().unwrap().rc_astro_ids = plan
            .nebula_chain
            .iter()
            .chain(plan.rgb_preparation.iter().flat_map(|p| &p.chain))
            .chain(&plan.rgb_chain)
            .cloned()
            .collect();
        assert!(state.stack_previews.insert_color(job.clone()));
        let progress = ColorProgressTracker {
            state: &state,
            job_id: &job.job_id,
        };
        let original: BTreeMap<_, _> = job
            .sources
            .iter()
            .map(|source| {
                let mut data: Vec<_> = (0..1024).map(|i| 0.1 + (i % 29) as f32 / 40.0).collect();
                if source.role == StackColorRole::Red {
                    data[0] = f32::NAN;
                }
                (source.role, LinearImage::new(32, 32, 1, data).unwrap())
            })
            .collect();
        for amount in [None, Some(3.0), Some(5.0)] {
            job.processing = Some(StackColorProcessing {
                narrowband: Some(NarrowbandProcessing {
                    rgb_star_stretch: amount,
                    star_reduction: if amount == Some(3.0) { 0.4 } else { 0.0 },
                    ..Default::default()
                }),
                ..Default::default()
            });
            let mut images = original.clone();
            let stars = separate_stars(
                &state,
                &job,
                &plan,
                root.path(),
                &mut images,
                &[],
                &progress,
                &ColorStop {
                    cancel: Stop::new(),
                },
                &|| Err("No test catalog".into()),
            )
            .unwrap()
            .unwrap();
            assert_eq!((stars.original.width, stars.starless.height), (32, 32));
            assert!(images[&StackColorRole::Ha].data[0].is_nan());
        }
        assert_eq!(
            tool.invocations(),
            3,
            "BXT and both separations must be reused on the second build"
        );
        assert_eq!(tool.invocations_log(), "bxt\nsxt\nsxt\n");
        let prepared = state
            .stack_previews
            .get_color(&job.job_id)
            .unwrap()
            .narrowband_resolution
            .unwrap()
            .rgb_star_processing
            .unwrap();
        assert_eq!(prepared.result.steps[0].tool, "bxt");
        let saved = state.stack_previews.get_color(&job.job_id).unwrap();
        let resolution = saved.narrowband_resolution.as_ref().unwrap();
        for (report, chain) in [
            (
                resolution.nebula_star_separation.as_ref().unwrap(),
                &plan.nebula_chain,
            ),
            (
                resolution.rgb_star_separation.as_ref().unwrap(),
                &plan.rgb_chain,
            ),
        ] {
            assert_eq!(&report.cache_id, chain.last().unwrap());
            assert!(report
                .output
                .stars_fits_url
                .as_ref()
                .unwrap()
                .ends_with("?stars=true"));
            assert!(super::super::super::rc_astro::rc_astro_chain_cached(
                root.path(),
                chain,
                &report.processing
            ));
        }
        // These exact references survive saving/reloading the final composition,
        // and protect the linear SXT outputs from the cache janitor.
        persist_color_manifest(root.path(), &saved).unwrap();
        persist_latest_color(root.path(), &saved).unwrap();
        let reloaded: StackColorJob = serde_json::from_slice(
            &std::fs::read(color_manifest_path(root.path(), &saved.job_id)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            reloaded
                .narrowband_resolution
                .as_ref()
                .unwrap()
                .rgb_star_separation
                .as_ref()
                .unwrap()
                .cache_id,
            *plan.rgb_chain.last().unwrap()
        );
        assert!(latest_color_rc_astro_ids(root.path()).contains(plan.rgb_chain.last().unwrap()));
        for id in &saved.narrowband_resolution.as_ref().unwrap().rc_astro_ids {
            let path =
                super::super::super::rc_astro::rc_astro_dir(root.path(), id).join("manifest.json");
            let file = std::fs::File::options().write(true).open(path).unwrap();
            file.set_modified(
                std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86400),
            )
            .unwrap();
        }
        super::super::super::rc_astro::prune_rc_astro_cache(root.path(), &Default::default());
        assert!(super::super::super::rc_astro::rc_astro_chain_cached(
            root.path(),
            &plan.rgb_chain,
            plan.rgb_config.as_ref().unwrap()
        ));
        assert!(super::super::super::rc_astro::rc_astro_chain_cached(
            root.path(),
            &plan.nebula_chain,
            &plan.config
        ));
        assert!(separation_artifacts_exist(
            root.path(),
            saved.narrowband_resolution.as_ref()
        ));
        std::fs::remove_file(
            super::super::super::rc_astro::rc_astro_dir(
                root.path(),
                plan.rgb_chain.last().unwrap(),
            )
            .join("stars.fits"),
        )
        .unwrap();
        assert!(!separation_artifacts_exist(
            root.path(),
            saved.narrowband_resolution.as_ref()
        ));
        separate_stars(
            &state,
            &job,
            &plan,
            root.path(),
            &mut original.clone(),
            &[],
            &progress,
            &ColorStop {
                cancel: Stop::new(),
            },
            &|| Err("No test catalog".into()),
        )
        .unwrap();
        assert_eq!(
            tool.invocations(),
            4,
            "Only the missing RGB stars sidecar must be repaired"
        );
        assert!(separation_artifacts_exist(
            root.path(),
            saved.narrowband_resolution.as_ref()
        ));
        // Retuning one branch must rerun that SXT only, keeping BXT and the
        // other branch's cached separation. Reapplying either reuses it too.
        let mut tuned = plan.clone();
        let mut rgb_config = tuned.rgb_config.clone().unwrap();
        rgb_config.steps[0].parameters.insert(
            "overlap".into(),
            seiza_stacking::ExternalParameterValue::Float(0.3),
        );
        tuned.rgb_chain = super::super::super::rc_astro::rc_astro_chain_ids(
            "db",
            "rgb",
            "first",
            &rgb_config,
            &tuned.schemas,
            None,
        )
        .unwrap();
        tuned.rgb_config = Some(rgb_config);
        for expected in [5, 5] {
            separate_stars(
                &state,
                &job,
                &tuned,
                root.path(),
                &mut original.clone(),
                &[],
                &progress,
                &ColorStop {
                    cancel: Stop::new(),
                },
                &|| Err("No test catalog".into()),
            )
            .unwrap();
            assert_eq!(tool.invocations(), expected);
        }
        tuned.config.steps[0].parameters.insert(
            "overlap".into(),
            seiza_stacking::ExternalParameterValue::Float(0.25),
        );
        tuned.nebula_chain = super::super::super::rc_astro::rc_astro_chain_ids(
            "db",
            "nebula",
            "first",
            &tuned.config,
            &tuned.schemas,
            None,
        )
        .unwrap();
        separate_stars(
            &state,
            &job,
            &tuned,
            root.path(),
            &mut original.clone(),
            &[],
            &progress,
            &ColorStop {
                cancel: Stop::new(),
            },
            &|| Err("No test catalog".into()),
        )
        .unwrap();
        assert_eq!(tool.invocations(), 6);
        tool.fail_star_removal();
        let mut failing_plan = plan;
        failing_plan.nebula_chain = vec!["b".repeat(64)];
        failing_plan.rgb_chain = vec!["c".repeat(64)];
        for choice in [ColorStars::Automatic, ColorStars::Rgb] {
            job.stars = Some(choice);
            let mut images = original.clone();
            let result = separate_stars(
                &state,
                &job,
                &failing_plan,
                root.path(),
                &mut images,
                &[],
                &progress,
                &ColorStop {
                    cancel: Stop::new(),
                },
                &|| Err("No test catalog".into()),
            );
            if choice == ColorStars::Automatic {
                assert!(result.unwrap().is_none());
                let saved = state.stack_previews.get_color(&job.job_id).unwrap();
                assert_eq!(
                    saved.narrowband_resolution.unwrap().stars,
                    ColorStars::Narrowband
                );
            } else {
                assert!(result.is_err());
            }
            assert_eq!(
                images[&StackColorRole::Ha].data,
                original[&StackColorRole::Ha].data
            );
        }
        let stop = ColorStop {
            cancel: Stop::new(),
        };
        stop.cancel.request();
        job.stars = Some(ColorStars::Automatic);
        assert!(
            separate_stars(
                &state,
                &job,
                &failing_plan,
                root.path(),
                &mut original.clone(),
                &[],
                &progress,
                &stop,
                &|| Err("No test catalog".into()),
            )
            .is_err(),
            "a stop must not become an automatic fallback"
        );
    }
}
