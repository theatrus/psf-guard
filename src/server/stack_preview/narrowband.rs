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
}

impl NarrowbandProcessing {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Some(options) = self.balance
            && (!options.shadow_protection.is_finite()
                || !(0.0..=1.0).contains(&options.shadow_protection)
                || [options.oiii_boost, options.sii_boost]
                    .iter()
                    .any(|v| !v.is_finite() || *v <= 0.0)
                || !(4..=1_000_000).contains(&options.max_samples))
        {
            return Err("Shadow protection must be 0 to 1, boosts positive and finite, and balance samples 4 to 1000000".into());
        }
        Ok(())
    }
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
}

pub(super) fn rgb_calibration_settings(job: &StackColorJob) -> Option<StackColorCalibration> {
    job.narrowband_resolution
        .as_ref()
        .filter(|resolution| resolution.stars == ColorStars::Rgb)
        .map(|_| StackColorCalibration::default())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StarsPlan {
    pub config: super::super::rc_astro::RcAstroProcessing,
    pub schemas: Vec<(String, seiza_stacking::ExternalToolSchema)>,
    pub nebula_chain: Vec<String>,
    pub rgb_chain: Vec<String>,
    pub calibration: Option<ColorCalibrationPlan>,
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
    Vec<(String, seiza_stacking::ExternalToolSchema)>,
);
type ResolvedSources = (
    Vec<StackColorSource>,
    Option<NarrowbandResolution>,
    Option<SeparationTool>,
);

pub(super) fn resolve(
    target: &TargetSources,
    request: &mut StackColorRequest,
) -> Result<ResolvedSources, AppError> {
    if request.kind != StackColorKind::Narrowband {
        return Ok((select_sources(target, request)?, None, None));
    }
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
    let schemas = match super::super::rc_astro::schemas_for(&config) {
        Ok(schemas) => schemas,
        Err(error) if choice == ColorStars::Automatic => {
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
    let config = match super::super::rc_astro::normalize_against_schemas(&config, &schemas) {
        Ok(config) => config,
        Err(error) if choice == ColorStars::Automatic => {
            resolution.stars = ColorStars::Narrowband;
            resolution.detail = format!(
                "Automatic uses narrowband stars: star separation is unavailable ({error})"
            );
            return Ok((sources, Some(resolution), None));
        }
        Err(error) => return Err(AppError::BadRequest(error)),
    };
    if choice != ColorStars::None {
        let rgb_request = StackColorRequest {
            kind: StackColorKind::Rgb,
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
            Ok(rgb) => sources.extend(rgb),
            Err(error) if choice == ColorStars::Automatic => {
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
        resolution.stars = ColorStars::Rgb;
        resolution.detail = "RGB stars replace narrowband stars on the starless nebula".into();
    } else {
        resolution.detail = "Starless narrowband nebula".into();
    }
    Ok((sources, Some(resolution), Some((config, schemas))))
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
    pub original: LinearImage,
    pub starless: LinearImage,
}

fn combine(
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

fn put_planes(
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
    let run = |chain: &[String], image: &LinearImage, label: &str| {
        super::super::rc_astro::apply_rc_astro(
            root,
            chain,
            &plan.config,
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
        let rgb = if let Some(calibration) = &plan.calibration {
            let outcome = match &calibration.placement {
                Ok(placement) => calibrate_registered_channels(
                    placement,
                    &calibration.settings,
                    images,
                    None,
                    open_catalog,
                ),
                Err(reason) => StackColorCalibrationOutcome::Skipped(reason.clone()),
            };
            state.stack_previews.update_color(&job.job_id, |current| {
                if let Some(resolution) = &mut current.narrowband_resolution {
                    resolution.color_calibration = Some(outcome);
                }
            });
            let original = combine(images, RGB_ROLES)?;
            let output = run(&plan.rgb_chain, &original, "RGB stars")?;
            stop.check()?;
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
        let mut output = run(&plan.nebula_chain, &physical, "Narrowband nebula")?.image;
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
            if job.stars != Some(ColorStars::Automatic) {
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
        resolve(&target, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Hoo));
        target.by_role.get_mut(&StackColorRole::Sii).unwrap()[0].exposure_group = target.by_role
            [&StackColorRole::Ha][0]
            .exposure_group
            .clone();
        target.by_role.get_mut(&StackColorRole::Sii).unwrap()[0].wbpp = true;
        resolve(&target, &mut request).unwrap();
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
        let (sources, _, _) = resolve(&target, &mut request).unwrap();
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
        let (sources, _, _) = resolve(&target, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Sho));
        assert!(sources.iter().any(|source| source.job_id == "sii-long"));
    }

    #[test]
    fn automatic_palette_uses_only_unambiguous_physical_filters() {
        let mut request = request(None);
        let target = target(false, true);
        resolve(&target, &mut request).unwrap();
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
        resolve(&ambiguous, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Hoo));
        request.input_sources.insert(
            StackColorRole::Sii,
            StackColorSourceRef {
                job_id: "Sii".into(),
                group_index: 0,
                artifact_revision: "first".into(),
            },
        );
        resolve(&ambiguous, &mut request).unwrap();
        assert_eq!(request.palette, Some(StackNarrowbandPalette::Sho));
        request
            .input_sources
            .get_mut(&StackColorRole::Sii)
            .unwrap()
            .artifact_revision = "stale".into();
        assert!(resolve(&ambiguous, &mut request).is_err());
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
            &target(true, true),
            &mut request(Some(ColorStars::Automatic)),
        )
        .unwrap();
        assert_eq!(sources.len(), 6);
        assert_eq!(result.unwrap().stars, ColorStars::Rgb);
        assert!(plan.is_some());
        let (sources, result, plan) = resolve(
            &target(false, true),
            &mut request(Some(ColorStars::Automatic)),
        )
        .unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(result.unwrap().stars, ColorStars::Narrowband);
        assert!(plan.is_none());
        assert!(resolve(&target(false, true), &mut request(Some(ColorStars::Rgb))).is_err());
        let (_, result, plan) =
            resolve(&target(false, true), &mut request(Some(ColorStars::None))).unwrap();
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
        let (selected, _, _) = resolve(&target, &mut request(Some(ColorStars::Automatic))).unwrap();
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
        let (selected, _, _) = resolve(&target, &mut request).unwrap();
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
    fn star_separation_reuses_both_caches_and_carries_rgb_coverage_into_the_crop() {
        let tool = super::super::super::rc_astro::test_tool::install(false);
        let (sources, resolution, config) =
            resolve(&target(true, true), &mut request(Some(ColorStars::Rgb))).unwrap();
        let (config, schemas) = config.unwrap();
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
        let plan = StarsPlan {
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
        for _ in 0..2 {
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
            2,
            "each tool result must be reused on the second build"
        );
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
