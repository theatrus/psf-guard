//! Combined recipes and continuum stages on exact aligned linear revisions.
use super::*;
use seiza_stacking::{
    ContinuumDiagnostics, ContinuumFitOptions, ContinuumFitOutcome, ContinuumOrigin,
    ContinuumPhotometryOptions, ContinuumPlan, ContinuumSource, ContinuumSources,
    ContinuumTransfer, EmissionBlendLine, EmissionBlendOptions, EmissionNegativePolicy,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CombinedColorRecipe {
    Emission,
    BroadbandStructure,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinuumMode {
    #[default]
    Automatic,
    Manual,
    Off,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmissionLineSettings {
    pub amount: f64,
    pub coefficients: Vec<f64>,
    pub reference_flux_ratios: Vec<f64>,
}
impl Default for EmissionLineSettings {
    fn default() -> Self {
        Self {
            amount: 0.25,
            coefficients: Vec::new(),
            reference_flux_ratios: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EmissionProcessing {
    pub mode: ContinuumMode,
    pub lines: BTreeMap<StackColorRole, EmissionLineSettings>,
    /// Only explicit physical working-unit limits; otherwise use producer clipping maps.
    pub saturation: BTreeMap<StackColorRole, f64>,
    pub fit: ContinuumFitOptions,
    pub photometry: ContinuumPhotometryOptions,
    pub positive_contribution: bool,
    pub broadband_amount: f64,
    /// Optional scientific export in calibrated physical units, independent of display amounts.
    pub flux_calibration: Option<seiza_stacking::EmissionFluxCalibration>,
}
impl Default for EmissionProcessing {
    fn default() -> Self {
        Self {
            mode: Default::default(),
            lines: [
                StackColorRole::Ha,
                StackColorRole::Oiii,
                StackColorRole::Sii,
            ]
            .into_iter()
            .map(|r| (r, Default::default()))
            .collect(),
            saturation: BTreeMap::new(),
            fit: Default::default(),
            photometry: Default::default(),
            positive_contribution: true,
            broadband_amount: 0.0,
            flux_calibration: None,
        }
    }
}
impl EmissionProcessing {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.mode == ContinuumMode::Automatic && self.photometry.max_pixel_residual.is_none() {
            return Err("Automatic continuum subtraction requires pixel profile validation".into());
        }
        if self.mode == ContinuumMode::Manual && self.photometry.psf_matching.is_some() {
            return Err("Use Automatic continuum mode for measured PSF matching, or disable matching before entering manual weights".into());
        }
        if !self.broadband_amount.is_finite()
            || !(0.0..=1.0).contains(&self.broadband_amount)
            || self.lines.is_empty()
            || self.lines.keys().any(|r| {
                !matches!(
                    r,
                    StackColorRole::Ha | StackColorRole::Oiii | StackColorRole::Sii
                )
            })
            || self.lines.values().any(|v| {
                !v.amount.is_finite()
                    || !(0.0..=5.0).contains(&v.amount)
                    || v.coefficients.iter().any(|c| !c.is_finite() || *c < 0.0)
                    || v.reference_flux_ratios
                        .iter()
                        .any(|r| !r.is_finite() || *r <= 0.0)
            })
            || self
                .saturation
                .values()
                .any(|v| !v.is_finite() || *v <= 0.0)
        {
            return Err("Emission amounts must be zero to five, broadband structure zero to one, and manual scales finite and positive".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContinuumLineResult {
    pub role: StackColorRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flux_cache_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flux_unit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flux_fits_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flux_report_url: Option<String>,
    pub references: Vec<StackColorRole>,
    pub status: String,
    pub detail: String,
    pub plan: Option<ContinuumPlan>,
    pub diagnostics: Option<ContinuumDiagnostics>,
    pub residual_fits_url: Option<String>,
    pub residual_preview_url: Option<String>,
    pub clipping_maps: bool,
    pub background_offset: Option<f64>,
    pub cache_id: Option<String>,
    pub report_url: Option<String>,
}

/// Producer masks can arrive after older stack artifacts. Track that change
/// without reading image planes while the request is being prepared.
pub(super) fn clipping_cache_revision(root: &FsPath, sources: &[StackColorSource]) -> String {
    let mut hash = Sha256::new();
    for source in sources {
        hash.update([source.role as u8]);
        let path = super::super::clipped_path(root, &source.job_id, source.group_index);
        if let Ok(metadata) = std::fs::metadata(path) {
            hash.update(metadata.len().to_le_bytes());
            hash.update(
                metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_nanos())
                    .to_le_bytes(),
            );
        } else {
            hash.update(b"missing");
        }
    }
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub(super) fn required(request: &StackColorRequest) -> Vec<StackColorRole> {
    let mut roles = required_roles(request.kind, request.palette);
    if let Some(recipe) = request.combination {
        match recipe {
            CombinedColorRecipe::Emission => {
                if let Some(config) = request
                    .processing
                    .as_ref()
                    .and_then(|p| p.emission.as_ref())
                {
                    roles.extend(config.lines.keys().copied());
                }
            }
            CombinedColorRecipe::BroadbandStructure => roles.extend(RGB_ROLES),
        }
    }
    roles.sort();
    roles.dedup();
    // Source order records the actual reference grid in the input cache key.
    let reference = reference_role(request.kind);
    if let Some(i) = roles.iter().position(|r| *r == reference) {
        roles.swap(0, i);
    }
    roles
}

pub(super) fn label(request: &StackColorRequest) -> String {
    let base = composition_label(request.kind, request.palette);
    match request.combination {
        Some(CombinedColorRecipe::Emission) => format!("{base} + emission"),
        Some(CombinedColorRecipe::BroadbandStructure) => format!("{base} + broadband"),
        None => base.into(),
    }
}

fn raw_path(root: &FsPath, input: &str, role: StackColorRole) -> PathBuf {
    color_input_dir(root, input).join(format!("linear-{}.fits", role_cache_name(role)))
}
pub(super) fn raw_artifacts_exist(root: &FsPath, input: &str, roles: &[StackColorRole]) -> bool {
    roles
        .iter()
        .all(|role| raw_path(root, input, *role).is_file())
}
pub(super) fn store_linear(
    root: &FsPath,
    input: &str,
    images: &BTreeMap<StackColorRole, LinearImage>,
    headers: &[(String, seiza_fits::HeaderValue)],
) -> Result<(), String> {
    for (role, image) in images {
        write_linear(&raw_path(root, input, *role), image, headers, *role)?;
    }
    Ok(())
}
fn write_linear(
    path: &FsPath,
    image: &LinearImage,
    headers: &[(String, seiza_fits::HeaderValue)],
    role: StackColorRole,
) -> Result<(), String> {
    let parent = path.parent().ok_or("Linear artifact has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp.fits", super::super::temporary_tag()));
    write_processed_image_fits_f32(
        &temporary,
        image,
        headers,
        &[
            seiza_fits::WriteHeaderCard::new(
                "SEIZATRF",
                seiza_fits::HeaderValue::String("LINEAR".into()),
            ),
            seiza_fits::WriteHeaderCard::new(
                "SEIZACON",
                seiza_fits::HeaderValue::String(
                    if path.file_name().is_some_and(|name| name == "residual.fits") {
                        "EMISSION-RESIDUAL"
                    } else if path
                        .parent()
                        .and_then(|p| p.parent())
                        .and_then(|p| p.file_name())
                        .is_some_and(|name| name == "emission-flux")
                    {
                        "CALIBRATED-EMISSION"
                    } else {
                        "ALIGNED-INPUT"
                    }
                    .into(),
                ),
            ),
            seiza_fits::WriteHeaderCard::new(
                "FILTER",
                seiza_fits::HeaderValue::String(role_cache_name(role).into()),
            ),
        ],
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}
fn cache_dir(root: &FsPath, input: &str, id: &str) -> PathBuf {
    color_input_dir(root, input).join("continuum").join(id)
}
fn result_urls(job: &StackColorJob, role: StackColorRole) -> (String, String, String) {
    let base = format!(
        "/api/db/{}/stack-previews/color/{}/continuum/{}",
        job.database_id,
        job.job_id,
        role_cache_name(role)
    );
    (
        format!("{base}/fits"),
        format!("{base}/preview"),
        format!("{base}/report"),
    )
}
fn compact(outcome: &ContinuumFitOutcome) -> (Option<ContinuumPlan>, ContinuumDiagnostics) {
    let (plan, mut diagnostics) = match outcome {
        ContinuumFitOutcome::Fitted { plan } => (
            Some(plan.clone()),
            plan.diagnostics.clone().unwrap_or_default(),
        ),
        ContinuumFitOutcome::Skipped { diagnostics, .. } => (None, diagnostics.clone()),
    };
    diagnostics.stars.clear();
    let plan = plan.map(|mut plan| {
        if let Some(d) = &mut plan.diagnostics {
            d.stars.clear();
        }
        plan
    });
    (plan, diagnostics)
}
fn clipping_map(
    root: &FsPath,
    source: &StackColorSource,
    transform: seiza_stacking::SimilarityTransform,
    grid: (usize, usize),
) -> Option<Vec<u8>> {
    let image = super::super::read_clipped(&super::super::clipped_path(
        root,
        &source.job_id,
        source.group_index,
    ))?;
    let aligned = if transform == seiza_stacking::SimilarityTransform::IDENTITY
        && (image.width, image.height) == grid
    {
        image
    } else {
        resample_to_reference(&image, grid.0, grid.1, transform).ok()?
    };
    Some(
        aligned
            .data
            .iter()
            .map(|v| u8::from(!v.is_finite() || *v > 0.0))
            .collect(),
    )
}

/// Fit and subtract exact pre-normalization images. The result is projected
/// into the existing input normalization before BXT/SXT or display stretches.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply(
    state: &Arc<AppState>,
    job: &StackColorJob,
    root: &FsPath,
    images: &mut BTreeMap<StackColorRole, LinearImage>,
    registered: &BTreeMap<StackColorRole, seiza_stacking::SimilarityTransform>,
    backgrounds: &BTreeMap<StackColorRole, BackgroundFit>,
    headers: &[(String, seiza_fits::HeaderValue)],
    progress: &ColorProgressTracker<'_>,
    stop: &ColorStop,
) -> Result<Option<LinearImage>, String> {
    let Some(config) = job.processing.as_ref().and_then(|p| p.emission.as_ref()) else {
        progress.skip(
            StackColorProgressPhase::MeasuringContinuum,
            "Continuum subtraction is off",
        );
        progress.skip(
            StackColorProgressPhase::SubtractingContinuum,
            "Continuum subtraction is off",
        );
        return Ok(None);
    };
    let recipe = job
        .combination
        .ok_or("Emission settings require an explicit combined recipe")?;
    let input = job
        .linear_input_id
        .as_deref()
        .ok_or("Combined recipes require retained aligned linear inputs")?;
    let mut raw = BTreeMap::new();
    let mut ranges = BTreeMap::new();
    let mut levels = BTreeMap::new();
    for source in &job.sources {
        stop.check()?;
        let frame = crate::image_io::open_linear_frame(raw_path(root, input, source.role))
            .map_err(|e| e.to_string())?;
        let (_, range) = super::super::stretch::normalize_linear_image(&frame.image)?;
        let level = backgrounds
            .get(&source.role)
            .and_then(|fit| fit.reference.first())
            .copied()
            .unwrap_or(f64::from(range.black));
        levels.insert(source.role, level);
        ranges.insert(source.role, range);
        raw.insert(source.role, frame.image);
    }
    let grid = (
        raw[&reference_role(job.kind)].width,
        raw[&reference_role(job.kind)].height,
    );
    let mut masks = BTreeMap::new();
    for source in &job.sources {
        if let Some(mask) = clipping_map(
            root,
            source,
            registered
                .get(&source.role)
                .copied()
                .unwrap_or(seiza_stacking::SimilarityTransform::IDENTITY),
            grid,
        ) {
            masks.insert(source.role, mask);
        }
    }
    let roles: Vec<_> = if recipe == CombinedColorRecipe::BroadbandStructure {
        composed_roles(job.kind, job.palette)
    } else {
        config.lines.keys().copied().collect()
    };
    let mut results = Vec::new();
    let mut residuals = Vec::new();
    let mut blend_lines = Vec::new();
    for role in roles {
        stop.check()?;
        let settings = config
            .lines
            .get(&role)
            .ok_or_else(|| format!("Add continuum settings for {}", role.label()))?;
        let references = if role == StackColorRole::Oiii {
            vec![StackColorRole::Green, StackColorRole::Blue]
        } else {
            vec![StackColorRole::Red]
        };
        let mut result = ContinuumLineResult {
            role,
            flux_cache_id: None,
            flux_unit: None,
            flux_fits_url: None,
            flux_report_url: None,
            references: references.clone(),
            status: "disabled".into(),
            detail: "Continuum subtraction off".into(),
            plan: None,
            diagnostics: None,
            residual_fits_url: None,
            residual_preview_url: None,
            clipping_maps: false,
            background_offset: None,
            cache_id: None,
            report_url: None,
        };
        if config.mode == ContinuumMode::Off
            || (recipe == CombinedColorRecipe::Emission
                && settings.amount == 0.0
                && config.flux_calibration.is_none())
        {
            results.push(result);
            continue;
        }
        progress.begin(
            StackColorProgressPhase::MeasuringContinuum,
            format!("Measuring {} continuum", role.label()),
            Some(role),
            None,
        );
        let source_for = |r: StackColorRole| -> ContinuumSource {
            let mask = masks.get(&r);
            let saturation = config.saturation.get(&r).copied();
            ContinuumSource {
                name: role_cache_name(r).into(),
                revision: format!("{input}:{}", role_cache_name(r)),
                transfer: ContinuumTransfer::Linear,
                saturation,
                clipping_revision: mask
                    .map(|m| fingerprint(m))
                    .or_else(|| saturation.map(|v| format!("explicit-working-ceiling:{v}"))),
            }
        };
        let sources = ContinuumSources {
            width: grid.0,
            height: grid.1,
            narrowband: source_for(role),
            references: references.iter().map(|r| source_for(*r)).collect(),
        };
        let payload = serde_json::to_vec(&(
            SEIZA_STACKING_OUTPUT_VERSION,
            &sources,
            config.mode,
            &config.fit,
            &config.photometry,
            &settings.coefficients,
            &settings.reference_flux_ratios,
        ))
        .map_err(|e| e.to_string())?;
        let id = fingerprint(&payload);
        let directory = cache_dir(root, input, &id);
        let report_path = directory.join("report.json");
        let fits_path = directory.join("residual.fits");
        let preview_path = directory.join("preview.png");
        result.cache_id = Some(id.clone());
        result.report_url = Some(result_urls(job, role).2);
        let cached = std::fs::read(&report_path)
            .ok()
            .and_then(|b| serde_json::from_slice::<ContinuumFitOutcome>(&b).ok())
            .filter(|outcome| match outcome {
                ContinuumFitOutcome::Fitted { plan } => {
                    plan.sources == sources && fits_path.is_file()
                }
                ContinuumFitOutcome::Skipped { .. } => true,
            });
        let raw_references: Vec<_> = references.iter().map(|r| &raw[r]).collect();
        let mut outcome = if let Some(cached) = cached {
            cached
        } else {
            let have_evidence = std::iter::once(&sources.narrowband)
                .chain(&sources.references)
                .all(|s| s.saturation.is_some() || s.clipping_revision.is_some());
            let measured = if have_evidence {
                let masks: Vec<_> = std::iter::once(&role)
                    .chain(&references)
                    .map(|r| {
                        masks
                            .get(r)
                            .cloned()
                            .unwrap_or_else(|| vec![0; grid.0 * grid.1])
                    })
                    .collect();
                let borrowed: Vec<_> = masks.iter().map(Vec::as_slice).collect();
                seiza_stacking::measure_and_fit_continuum_masked(
                    &raw[&role],
                    &raw_references,
                    &sources,
                    &borrowed,
                    &[],
                    &config.fit,
                    &config.photometry,
                )
                .map_err(|e| e.to_string())?
            } else {
                ContinuumFitOutcome::Skipped{reason:seiza_stacking::ContinuumSkipReason::UnknownSaturation,
                    detail:"Producer clipping maps or explicit physical working-unit limits are required for every input".into(),diagnostics:Default::default()}
            };
            if config.mode == ContinuumMode::Manual {
                if settings.coefficients.len() != references.len() {
                    return Err(format!(
                        "Enter {} manual continuum weights for {}",
                        references.len(),
                        role.label()
                    ));
                }
                let ratios = if !settings.reference_flux_ratios.is_empty() {
                    settings.reference_flux_ratios.clone()
                } else {
                    match &measured {
                        ContinuumFitOutcome::Fitted { plan } => plan.reference_flux_ratios.clone(),
                        ContinuumFitOutcome::Skipped { diagnostics, .. } => {
                            diagnostics.reference_flux_ratios.clone()
                        }
                    }
                };
                ContinuumFitOutcome::Fitted {
                    plan: ContinuumPlan {
                        schema_version: 1,
                        psf_matching: None,
                        sources: sources.clone(),
                        coefficients: settings.coefficients.clone(),
                        reference_flux_ratios: ratios,
                        origin: ContinuumOrigin::Manual,
                        diagnostics: None,
                    },
                }
            } else {
                measured
            }
        };
        if let ContinuumFitOutcome::Fitted { plan } = &mut outcome {
            progress.begin(
                StackColorProgressPhase::SubtractingContinuum,
                format!("Subtracting {} continuum", role.label()),
                Some(role),
                None,
            );
            let residual = if fits_path.is_file() {
                crate::image_io::open_linear_frame(&fits_path)
                    .map_err(|e| e.to_string())?
                    .image
            } else {
                let residual = seiza_stacking::subtract_continuum(
                    &raw[&role],
                    &raw_references,
                    &sources,
                    plan,
                )
                .map_err(|e| e.to_string())?;
                write_linear(&fits_path, &residual, headers, role)?;
                residual
            };
            if !preview_path.is_file() {
                let _ = super::super::stretch::render_image_preview_atomic(
                    &residual,
                    &super::super::stretch::default_linear_config(),
                    super::super::stretch::StackStretchSourceTransfer::Linear,
                    &preview_path,
                );
            }
            let (fits_url, preview_url, _) = result_urls(job, role);
            result.residual_fits_url = Some(fits_url);
            result.residual_preview_url = preview_path.is_file().then_some(preview_url);
            let offset = levels[&role]
                - plan
                    .coefficients
                    .iter()
                    .zip(&references)
                    .map(|(a, r)| a * levels[r])
                    .sum::<f64>();
            result.background_offset = Some(offset);
            result.status = if plan.origin == ContinuumOrigin::Manual {
                "manual"
            } else {
                "applied"
            }
            .into();
            result.detail =
                "Signed residual retained; display baseline propagated separately".into();
            result.clipping_maps = std::iter::once(&role)
                .chain(&references)
                .all(|r| masks.contains_key(r));
            if recipe == CombinedColorRecipe::Emission {
                blend_lines.push(EmissionBlendLine {
                    plan: plan.clone(),
                    reference_channels: references
                        .iter()
                        .map(|r| {
                            RGB_ROLES
                                .iter()
                                .position(|x| x == r)
                                .expect("RGB reference")
                        })
                        .collect(),
                    amount: settings.amount,
                    background_offset: offset,
                });
                residuals.push(residual);
            } else {
                let range = &ranges[&role];
                let domain = seiza_stretch::ResolvedSampleDomain::PhysicalLinear {
                    black: offset as f32,
                    white: (offset + f64::from(range.white - range.black)) as f32,
                };
                let data = domain.apply(&residual.data, 1).map_err(|e| e.to_string())?;
                images.insert(
                    role,
                    LinearImage::new(grid.0, grid.1, 1, data).map_err(|e| e.to_string())?,
                );
            }
            progress.advance(StackColorProgressPhase::SubtractingContinuum, 1);
        } else if let ContinuumFitOutcome::Skipped { detail, .. } = &outcome {
            result.status = "skipped".into();
            result.detail = detail.clone();
        }
        let (plan, diagnostics) = compact(&outcome);
        result.plan = plan;
        result.diagnostics = Some(diagnostics);
        std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        write_json_atomic(&report_path, &outcome)?;
        progress.advance(StackColorProgressPhase::MeasuringContinuum, 1);
        results.push(result);
        state
            .stack_previews
            .update_color(&job.job_id, |current| current.continuum = results.clone());
    }
    if let Some(calibration) = &config.flux_calibration {
        let mut selected = Vec::new();
        for name in &calibration.lines {
            let index = results
                .iter()
                .position(|r| role_cache_name(r.role) == name)
                .ok_or_else(|| {
                    format!("Calibrated flux line {name} is not part of this combination")
                })?;
            if results[index].plan.is_none() {
                return Err(format!("Calibrated flux export needs a validated residual for {name}; inspect the continuum report"));
            }
            selected.push(index);
        }
        let plans: Vec<_> = selected
            .iter()
            .map(|i| results[*i].plan.clone().unwrap())
            .collect();
        let flux_id = fingerprint(
            &serde_json::to_vec(&(SEIZA_STACKING_OUTPUT_VERSION, &plans, calibration))
                .map_err(|e| e.to_string())?,
        );
        let flux_directory = color_input_dir(root, input)
            .join("emission-flux")
            .join(&flux_id);
        let residuals = selected
            .iter()
            .map(|i| {
                let id = results[*i]
                    .cache_id
                    .as_deref()
                    .ok_or("Continuum cache identity is missing")?;
                crate::image_io::open_linear_frame(cache_dir(root, input, id).join("residual.fits"))
                    .map(|f| f.image)
                    .map_err(|e| e.to_string())
            })
            .collect::<Result<Vec<_>, String>>()?;
        let (fluxes, report) = seiza_stacking::solve_emission_flux(
            &residuals.iter().collect::<Vec<_>>(),
            &plans,
            calibration,
        )
        .map_err(|e| format!("Calibrated flux export: {e}"))?;
        let mut flux_headers = headers.to_vec();
        flux_headers.retain(|(key, _)| !key.eq_ignore_ascii_case("BUNIT"));
        flux_headers.push((
            "BUNIT".into(),
            seiza_fits::HeaderValue::String(report.unit.clone()),
        ));
        for (index, flux) in selected.iter().zip(fluxes) {
            let result = &mut results[*index];
            write_linear(
                &flux_directory.join(format!("{}.fits", role_cache_name(result.role))),
                &flux,
                &flux_headers,
                result.role,
            )?;
            let base = format!(
                "/api/db/{}/stack-previews/color/{}/continuum/{}",
                job.database_id,
                job.job_id,
                role_cache_name(result.role)
            );
            result.flux_cache_id = Some(flux_id.clone());
            result.flux_unit = Some(report.unit.clone());
            result.flux_fits_url = Some(format!("{base}/flux"));
            result.flux_report_url = Some(format!("{base}/flux-report"));
        }
        write_json_atomic(&flux_directory.join("report.json"), &report)?;
    }
    if recipe == CombinedColorRecipe::BroadbandStructure
        && results.iter().any(|r| r.status == "skipped")
    {
        return Err("A required mapped line failed continuum validation. Review its fit, choose Manual or Off, or choose another palette; the previous result is retained".into());
    }
    let contribution = if recipe == CombinedColorRecipe::Emission && !blend_lines.is_empty() {
        let zero = LinearImage::new(grid.0, grid.1, 3, vec![0.0; grid.0 * grid.1 * 3])
            .map_err(|e| e.to_string())?;
        let borrowed: Vec<_> = residuals.iter().collect();
        let projected = seiza_stacking::blend_emission_rgb(
            &zero,
            &borrowed,
            &blend_lines,
            &EmissionBlendOptions {
                negative: if config.positive_contribution {
                    EmissionNegativePolicy::PositiveContribution
                } else {
                    EmissionNegativePolicy::Preserve
                },
            },
        )
        .map_err(|e| e.to_string())?;
        let mut planes = Vec::new();
        for (channel, role) in RGB_ROLES.iter().enumerate() {
            let range = &ranges[role];
            let plane: Vec<_> = projected
                .data
                .iter()
                .skip(channel)
                .step_by(3)
                .copied()
                .collect();
            let data = seiza_stretch::ResolvedSampleDomain::PhysicalLinear {
                black: 0.0,
                white: range.white - range.black,
            }
            .apply(&plane, 1)
            .map_err(|e| e.to_string())?;
            planes.push(LinearImage::new(grid.0, grid.1, 1, data).map_err(|e| e.to_string())?);
        }
        Some(
            combine_rgb(
                &planes[0],
                &planes[1],
                &planes[2],
                &ColorOptions {
                    normalization: ColorNormalization::None,
                    ..Default::default()
                },
            )
            .map_err(|e| e.to_string())?
            .image,
        )
    } else {
        None
    };
    let applied = results
        .iter()
        .any(|r| matches!(r.status.as_str(), "applied" | "manual"));
    state
        .stack_previews
        .update_color(&job.job_id, |current| current.continuum = results);
    if config.mode == ContinuumMode::Off {
        progress.skip(
            StackColorProgressPhase::MeasuringContinuum,
            "Continuum fitting is off",
        );
        progress.skip(
            StackColorProgressPhase::SubtractingContinuum,
            "Continuum subtraction is off",
        );
    } else {
        progress.finish(StackColorProgressPhase::MeasuringContinuum);
        if applied {
            progress.finish(StackColorProgressPhase::SubtractingContinuum);
        } else {
            progress.skip(
                StackColorProgressPhase::SubtractingContinuum,
                "No validated continuum line to subtract",
            );
        }
    }
    Ok(contribution)
}

pub(super) fn artifacts_exist(root: &FsPath, job: &StackColorJob) -> bool {
    job.continuum.iter().all(|result| {
        let Some(id) = &result.cache_id else {
            return true;
        };
        let Some(input) = &job.linear_input_id else {
            return false;
        };
        if super::super::validate_job_id(id).is_err()
            || super::super::validate_job_id(input).is_err()
        {
            return false;
        }
        let directory = cache_dir(root, input, id);
        let flux_exists = result.flux_cache_id.as_ref().is_none_or(|flux| {
            super::super::validate_job_id(flux).is_ok() && {
                let path = color_input_dir(root, input)
                    .join("emission-flux")
                    .join(flux);
                path.join(format!("{}.fits", role_cache_name(result.role)))
                    .is_file()
                    && path.join("report.json").is_file()
            }
        });
        flux_exists
            && directory.join("report.json").is_file()
            && (result.residual_fits_url.is_none() || directory.join("residual.fits").is_file())
            && (result.residual_preview_url.is_none() || directory.join("preview.png").is_file())
    })
}

fn artifact_job(
    state: &Arc<AppState>,
    root: &FsPath,
    database: &str,
    id: &str,
) -> Result<StackColorJob, AppError> {
    let job = state
        .stack_previews
        .get_color(id)
        .or_else(|| {
            std::fs::read(color_manifest_path(root, id))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<StackColorJob>(&bytes).ok())
        })
        .ok_or(AppError::NotFound)?;
    if job.database_id != database {
        return Err(AppError::NotFound);
    }
    Ok(job)
}

pub(super) async fn artifact(
    state: &Arc<AppState>,
    ctx: DbContext,
    job_id: String,
    role: String,
    kind: String,
) -> Result<Response, AppError> {
    super::super::validate_job_id(&job_id)?;
    let role = match role.as_str() {
        "ha" => StackColorRole::Ha,
        "oiii" => StackColorRole::Oiii,
        "sii" => StackColorRole::Sii,
        _ => return Err(AppError::NotFound),
    };
    let job = artifact_job(state, &ctx.stack_root, &ctx.id, &job_id)?;
    let result = job
        .continuum
        .iter()
        .find(|result| result.role == role)
        .ok_or(AppError::NotFound)?;
    let input = job.linear_input_id.as_deref().ok_or(AppError::NotFound)?;
    let id = result.cache_id.as_deref().ok_or(AppError::NotFound)?;
    super::super::validate_job_id(input)?;
    super::super::validate_job_id(id)?;
    let directory = cache_dir(&ctx.stack_root, input, id);
    if matches!(kind.as_str(), "flux" | "flux-report") {
        let id = result.flux_cache_id.as_deref().ok_or(AppError::NotFound)?;
        super::super::validate_job_id(id)?;
        let directory = color_input_dir(&ctx.stack_root, input)
            .join("emission-flux")
            .join(id);
        return if kind == "flux" {
            stream_artifact(
                directory.join(format!("{}.fits", role_cache_name(role))),
                "application/fits",
                Some(format!(
                    "psf-guard-{}-calibrated-flux.fits",
                    role_cache_name(role)
                )),
            )
            .await
        } else {
            stream_artifact(directory.join("report.json"), "application/json", None).await
        };
    }
    match kind.as_str() {
        "fits" => {
            stream_artifact(
                directory.join("residual.fits"),
                "application/fits",
                Some(format!(
                    "psf-guard-{}-signed-residual.fits",
                    role_cache_name(role)
                )),
            )
            .await
        }
        "preview" => stream_artifact(directory.join("preview.png"), "image/png", None).await,
        "report" => stream_artifact(directory.join("report.json"), "application/json", None).await,
        _ => Err(AppError::NotFound),
    }
}

fn fingerprint(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Natural combined recipes retain broadband stars; SXT has no restoration stage.
pub(super) fn validate_natural_star_removal(
    processing: &StackColorProcessing,
) -> Result<(), AppError> {
    if processing
        .input_rc_astro
        .values()
        .any(|config| config.steps.iter().any(|step| step.tool == "sxt"))
    {
        return Err(AppError::BadRequest("RGB/LRGB with emission retains broadband stars and cannot use channel StarXTerminator. Choose Set in this build and remove SXT, or choose narrowband with RGB stars.".into()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::tests::{channel, running_color_job};
    use super::*;
    fn setup(
        root: &FsPath,
        mode: ContinuumMode,
    ) -> (
        Arc<AppState>,
        StackColorJob,
        BTreeMap<StackColorRole, LinearImage>,
    ) {
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        let mut job = running_color_job(&"a".repeat(64), StackJobState::Running);
        job.kind = StackColorKind::Rgb;
        job.palette = None;
        job.combination = Some(CombinedColorRecipe::Emission);
        job.linear_input_id = Some("b".repeat(64));
        job.sources = [
            StackColorRole::Red,
            StackColorRole::Green,
            StackColorRole::Blue,
            StackColorRole::Ha,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, role)| channel(role, role_cache_name(role), &format!("{i:064x}"), false))
        .collect();
        let config = EmissionProcessing {
            mode,
            lines: BTreeMap::from([(
                StackColorRole::Ha,
                EmissionLineSettings {
                    amount: 0.25,
                    coefficients: vec![0.2],
                    reference_flux_ratios: vec![],
                },
            )]),
            ..Default::default()
        };
        job.processing = Some(StackColorProcessing {
            emission: Some(config),
            ..Default::default()
        });
        let images = job
            .sources
            .iter()
            .map(|source| {
                let data = if source.role == StackColorRole::Ha {
                    vec![1.0, 2.0, 4.0, 0.5]
                } else {
                    vec![10.0, 20.0, 30.0, 40.0]
                };
                (source.role, LinearImage::new(4, 1, 1, data).unwrap())
            })
            .collect();
        store_linear(root, job.linear_input_id.as_deref().unwrap(), &images, &[]).unwrap();
        assert!(state.stack_previews.insert_color(job.clone()));
        (state, job, images)
    }
    fn run(
        state: &Arc<AppState>,
        job: &StackColorJob,
        root: &FsPath,
        images: &mut BTreeMap<StackColorRole, LinearImage>,
    ) -> Result<Option<LinearImage>, String> {
        apply(
            state,
            job,
            root,
            images,
            &BTreeMap::new(),
            &BTreeMap::new(),
            &[],
            &ColorProgressTracker {
                state,
                job_id: &job.job_id,
            },
            &ColorStop {
                cancel: Stop::new(),
            },
        )
    }
    #[test]
    fn manual_residuals_keep_signed_pixels_and_amount_changes_reuse_the_fit() {
        let root = tempfile::tempdir().unwrap();
        let (state, job, mut images) = setup(root.path(), ContinuumMode::Manual);
        let before = images[&StackColorRole::Red].data.clone();
        assert!(run(&state, &job, root.path(), &mut images)
            .unwrap()
            .is_some());
        assert_eq!(images[&StackColorRole::Red].data, before);
        let first = state.stack_previews.get_color(&job.job_id).unwrap();
        let line = &first.continuum[0];
        assert_eq!(line.status, "manual");
        assert!(artifacts_exist(root.path(), &first));
        let path = cache_dir(
            root.path(),
            job.linear_input_id.as_deref().unwrap(),
            line.cache_id.as_deref().unwrap(),
        )
        .join("residual.fits");
        let residual = crate::image_io::open_linear_frame(&path).unwrap();
        assert!(residual.image.data.iter().all(|v| *v < 0.0));
        assert!(residual.headers.iter().any(|(k, v)| k == "SEIZACON"
            && matches!(v,seiza_fits::HeaderValue::String(v) if v=="EMISSION-RESIDUAL")));
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        let mut changed = job.clone();
        changed
            .processing
            .as_mut()
            .unwrap()
            .emission
            .as_mut()
            .unwrap()
            .lines
            .get_mut(&StackColorRole::Ha)
            .unwrap()
            .amount = 0.5;
        run(&state, &changed, root.path(), &mut images).unwrap();
        let second = state.stack_previews.get_color(&job.job_id).unwrap();
        assert_eq!(line.cache_id, second.continuum[0].cache_id);
        assert_eq!(
            modified,
            std::fs::metadata(path).unwrap().modified().unwrap()
        );
    }
    #[test]
    fn unknown_saturation_skips_natural_emission_and_stops_required_mapped_lines() {
        let root = tempfile::tempdir().unwrap();
        let (state, mut job, mut images) = setup(root.path(), ContinuumMode::Automatic);
        assert!(run(&state, &job, root.path(), &mut images)
            .unwrap()
            .is_none());
        let first = state.stack_previews.get_color(&job.job_id).unwrap();
        assert_eq!(first.continuum[0].status, "skipped");
        assert!(first.continuum[0].detail.contains("clipping"));
        job.kind = StackColorKind::Narrowband;
        job.palette = Some(StackNarrowbandPalette::Hoo);
        job.combination = Some(CombinedColorRecipe::BroadbandStructure);
        job.processing
            .as_mut()
            .unwrap()
            .emission
            .as_mut()
            .unwrap()
            .lines
            .insert(StackColorRole::Oiii, Default::default());
        let mut oiii = job.sources.last().unwrap().clone();
        oiii.role = StackColorRole::Oiii;
        job.sources.push(oiii);
        images.insert(StackColorRole::Oiii, images[&StackColorRole::Ha].clone());
        store_linear(
            root.path(),
            job.linear_input_id.as_deref().unwrap(),
            &images,
            &[],
        )
        .unwrap();
        assert!(run(&state, &job, root.path(), &mut images)
            .unwrap_err()
            .contains("previous result is retained"));
    }
    #[test]
    fn calibrated_flux_is_a_signed_scoped_export_and_keeps_display_amounts() {
        let root = tempfile::tempdir().unwrap();
        let (state, mut job, mut images) = setup(root.path(), ContinuumMode::Manual);
        let baseline = run(&state, &job, root.path(), &mut images)
            .unwrap()
            .unwrap();
        let initial = state.stack_previews.get_color(&job.job_id).unwrap();
        let plan = initial.continuum[0].plan.as_ref().unwrap();
        job.processing
            .as_mut()
            .unwrap()
            .emission
            .as_mut()
            .unwrap()
            .flux_calibration = Some(seiza_stacking::EmissionFluxCalibration {
            revision: "response-v1".into(),
            unit: "flux/pixel".into(),
            lines: vec!["ha".into()],
            sources: vec![plan.sources.clone()],
            narrowband_response: vec![vec![1.0]],
            continuum_coefficients: vec![vec![0.2]],
            residual_zero_levels: vec![0.0],
            reference_response: vec![vec![vec![0.5]]],
            residual_covariance: None,
        });
        let combined = run(&state, &job, root.path(), &mut images)
            .unwrap()
            .unwrap();
        assert_eq!(baseline.data, combined.data);
        let saved = state.stack_previews.get_color(&job.job_id).unwrap();
        assert!(artifacts_exist(root.path(), &saved));
        let line = &saved.continuum[0];
        assert_eq!(line.flux_unit.as_deref(), Some("flux/pixel"));
        let directory = color_input_dir(root.path(), job.linear_input_id.as_deref().unwrap())
            .join("emission-flux")
            .join(line.flux_cache_id.as_deref().unwrap());
        let flux = crate::image_io::open_linear_frame(directory.join("ha.fits")).unwrap();
        assert!((flux.image.data[0] + 1.0 / 0.9).abs() < 1e-6);
        assert!(flux.headers.iter().any(|(key, value)| key == "BUNIT"
            && matches!(value,seiza_fits::HeaderValue::String(unit) if unit=="flux/pixel")));
        let report: seiza_stacking::EmissionFluxReport =
            serde_json::from_slice(&std::fs::read(directory.join("report.json")).unwrap()).unwrap();
        assert!(report.flux_covariance.is_none());
        assert_eq!(report.plans[0].coefficients, vec![0.2]);
        std::fs::remove_file(directory.join("ha.fits")).unwrap();
        assert!(!artifacts_exist(root.path(), &saved));
    }

    #[test]
    fn natural_combined_sxt_intent_is_rejected_for_own_or_inherited_processing() {
        let mut processing = StackColorProcessing::default();
        let sxt =
            serde_json::from_value(serde_json::json!({"steps":[{"tool":"sxt","parameters":{}}]}))
                .unwrap();
        processing.input_rc_astro.insert(StackColorRole::Red, sxt);
        for mode in [ColorInputProcessing::Own, ColorInputProcessing::Channels] {
            processing.input_processing = mode;
            assert!(validate_natural_star_removal(&processing).is_err());
        }
        processing.input_rc_astro.clear();
        assert!(validate_natural_star_removal(&processing).is_ok());
    }

    #[test]
    fn failed_fit_reports_are_available_before_any_completed_manifest() {
        let root = tempfile::tempdir().unwrap();
        let (state, job, mut images) = setup(root.path(), ContinuumMode::Automatic);
        run(&state, &job, root.path(), &mut images).unwrap();
        state
            .stack_previews
            .update_color(&job.job_id, |job| job.state = StackJobState::Failed);
        let failed = artifact_job(&state, root.path(), &job.database_id, &job.job_id).unwrap();
        assert_eq!(failed.state, StackJobState::Failed);
        assert!(failed.continuum[0].report_url.is_some());
        assert!(artifact_job(&state, root.path(), "another-db", &job.job_id).is_err());
    }

    #[test]
    fn combined_recipes_require_their_explicit_kind_and_star_policy() {
        let mut request:StackColorRequest=serde_json::from_value(serde_json::json!({"target_id":1,"kind":"rgb","combination":"emission","processing":{"emission":{}}})).unwrap();
        assert!(validate_request(&request).is_ok());
        request.kind = StackColorKind::Narrowband;
        request.palette = Some(StackNarrowbandPalette::Sho);
        assert!(validate_request(&request).is_err());
        request.combination = Some(CombinedColorRecipe::BroadbandStructure);
        assert!(validate_request(&request).is_err());
        request.stars = Some(narrowband::ColorStars::Rgb);
        assert!(validate_request(&request).is_ok());
    }
}
