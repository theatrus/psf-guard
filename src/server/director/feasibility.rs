//! Night-by-night feasibility for a project at each rig that could shoot
//! it: dark hours, hours the target sits inside the rig's limits and above
//! its horizon, the Moon, and the curves behind the first night. Planning
//! estimates from the shared core; the rig still decides at dispatch time.

use super::*;
use psf_guard_director_core::{
    night::{night_curve, night_preview, Night, NightCurve, NightRequest, NightTarget},
    visibility::{AltitudeLimits, Horizon, IcrsPosition, Site},
};
use psf_guard_director_meta::{plan::Goal, profile::Limits};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_NIGHTS: u32 = 7;
const STEP_MS: u64 = 300_000;
const DARK_BELOW_DEGREES: f64 = -12.0;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    #[serde(default)]
    nights: Option<u32>,
    /// The center to evaluate; defaults to the saved framing's center.
    #[serde(default)]
    center: Option<IcrsPosition>,
    #[serde(default)]
    start_ms: Option<u64>,
}

#[derive(Serialize)]
struct RigFeasibility {
    rig: NamedIdentity,
    catalog_name: String,
    site: Site,
    custom_horizon: bool,
    limits: Limits,
    nights: Vec<Night>,
    /// The first night in full, for the altitude chart.
    curve: NightCurve,
    /// Hours this rig owes the plan, from its enabled contributions.
    hours_needed: Option<f64>,
    /// At this week's average, how many nights until it is done.
    nights_to_complete: Option<u32>,
    /// Whether the plan has this rig ticked; unticked rigs still show.
    in_plan: bool,
}

#[derive(Serialize)]
pub(super) struct View {
    center: IcrsPosition,
    target_name: String,
    nights: u32,
    rigs: Vec<RigFeasibility>,
    warnings: Vec<String>,
}

pub(super) enum FeasibilityError {
    Api(Error),
    NotReady(String),
}
impl From<Error> for FeasibilityError {
    fn from(error: Error) -> Self {
        Self::Api(error)
    }
}
impl From<StoreError> for FeasibilityError {
    fn from(error: StoreError) -> Self {
        Self::Api(error.into())
    }
}
impl IntoResponse for FeasibilityError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::NotReady(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ApiResponse::<()>::error(message)),
            )
                .into_response(),
        }
    }
}

pub(super) async fn evaluate(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<Request>,
) -> Result<Json<ApiResponse<View>>, FeasibilityError> {
    let service = enabled(&state)?;
    let nights = request.nights.unwrap_or(DEFAULT_NIGHTS);
    if nights == 0 || nights > 14 {
        return Err(Error::Invalid.into());
    }
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let permit = admit(&service.admission).await?;
    let view = tokio::task::spawn_blocking(move || {
        // Reads happen under the store lock and the admission permit; the
        // night curves, which take the time, run after both are released so
        // the rest of the workspace is not kept waiting behind them.
        let store = service.store.lock().map_err(|_| Error::Internal)?;
        store.project(id)?.ok_or(Error::Missing)?;
        let framing = store.framing_draft(id)?;
        let center = request
            .center
            .or_else(|| framing.as_ref().map(|f| f.center))
            .ok_or_else(|| FeasibilityError::NotReady("Frame the project or send a center first.".into()))?;
        if !(0.0..360.0).contains(&center.ra_degrees) || !(-90.0..=90.0).contains(&center.dec_degrees) {
            return Err(Error::Invalid.into());
        }
        let target_name = framing
            .as_ref()
            .map(|f| f.target_name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Target".into());
        let plan = store.plan_draft(id)?;
        // A contribution covers every panel unless it names some; the rig
        // shoots each panel's frames, so the hours owed scale with panels.
        let panels = framing
            .as_ref()
            .map(|f| (f.mosaic.rows * f.mosaic.columns).max(1))
            .unwrap_or(1);
        let start_ms = request.start_ms.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0)
        });
        // Every rig with a site; the plan's rigs first.
        let mut warnings = Vec::new();
        let mut rigs = Vec::new();
        let mut names = std::collections::BTreeMap::new();
        let found = identified_catalogs(&catalogs, service.instance_id);
        warnings.extend(found.duplicates.iter().cloned());
        for (identity, catalog) in found.iter() {
            if let Some(binding) = store.catalog_rig(identity.id)? {
                names.insert(binding.rig.id, (binding.rig, catalog.name.clone()));
            }
        }
        let mut inputs = Vec::new();
        for (rig_id, (rig, catalog_name)) in names {
            match store.rig_profile(rig_id)? {
                Some(profile) => inputs.push((rig_id, rig, catalog_name, profile)),
                None => warnings.push(format!("{catalog_name}: no rig profile yet; set its site under Setup.")),
            }
        }
        // Everything below is arithmetic on what was read: let the next
        // request at the store go while the night curves are computed.
        drop(store);
        drop(permit);
        for (rig_id, rig, catalog_name, profile) in inputs {
            let Some(site) = profile.site.as_ref().map(|s| s.value) else {
                warnings.push(format!("{catalog_name}: no site in its rig profile; set it under Setup or let the plugin report it."));
                continue;
            };
            let horizon = profile
                .horizon
                .as_ref()
                .map(|h| h.value.clone())
                .unwrap_or(Horizon::FixedMinimum {});
            let limits = profile.limits.value;
            let night_request = NightRequest {
                site,
                horizon: horizon.clone(),
                limits: AltitudeLimits {
                    rig_minimum_degrees: limits.minimum_altitude_degrees,
                    rig_maximum_degrees: limits.maximum_altitude_degrees,
                    project_minimum_degrees: -90.0,
                    project_maximum_degrees: 90.0,
                    horizon_offset_degrees: 0.0,
                },
                targets: vec![NightTarget {
                    id: "center".into(),
                    position: center,
                }],
                start_ms,
                nights,
                step_ms: STEP_MS,
                dark_below_degrees: DARK_BELOW_DEGREES,
                meridian_exclusion: limits.meridian_exclusion,
            };
            let nights_out = match night_preview(&night_request) {
                Ok(nights) => nights,
                Err(error) => {
                    warnings.push(format!("{catalog_name}: feasibility could not be computed ({error:?})."));
                    continue;
                }
            };
            let curve = match night_curve(&night_request, 0) {
                Ok(curve) => curve,
                Err(error) => {
                    warnings.push(format!("{catalog_name}: tonight's curve could not be computed ({error:?})."));
                    continue;
                }
            };
            let contributions: Vec<_> = plan
                .as_ref()
                .map(|p| p.contributions.iter().filter(|c| c.enabled && c.rig_id == rig_id).collect())
                .unwrap_or_default();
            let in_plan = !contributions.is_empty();
            let hours_needed = plan.as_ref().and_then(|p| {
                let mut total = 0.0;
                for c in &contributions {
                    let objective = p.objectives.iter().find(|o| o.id == c.objective_id)?;
                    let frames = match objective.goal {
                        Goal::Hours { value } => (value * 3600.0 / c.exposure_seconds).ceil(),
                        Goal::Frames { value } => f64::from(value),
                    };
                    let panel_count = if c.panel_ids.is_empty() {
                        panels
                    } else {
                        c.panel_ids.len() as u32
                    };
                    total += frames * c.exposure_seconds / 3600.0 * f64::from(panel_count);
                }
                in_plan.then_some(total)
            });
            let mean_hours_up = nights_out
                .iter()
                .map(|n| n.targets.first().map_or(0.0, |t| t.hours_up))
                .sum::<f64>()
                / nights_out.len().max(1) as f64;
            let nights_to_complete = hours_needed
                .filter(|_| mean_hours_up > 0.05)
                .map(|needed| (needed / mean_hours_up).ceil() as u32);
            rigs.push(RigFeasibility {
                rig,
                catalog_name,
                site,
                custom_horizon: matches!(horizon, Horizon::Custom { .. }),
                limits,
                nights: nights_out,
                curve,
                hours_needed,
                nights_to_complete,
                in_plan,
            });
        }
        rigs.sort_by(|a, b| b.in_plan.cmp(&a.in_plan).then_with(|| a.catalog_name.cmp(&b.catalog_name)));
        if rigs.is_empty() {
            warnings.push("No rig has a site yet, so nothing can be timed.".into());
        }
        Ok::<_, FeasibilityError>(View {
            center,
            target_name,
            nights,
            rigs,
            warnings,
        })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director feasibility worker failed");
        Error::Internal
    })??;
    Ok(Json(ApiResponse::success(view)))
}
