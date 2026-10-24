//! The server's scoring defaults: how hard each kind of event evidence hits
//! a frame's quality score, and the absolute reject limits. One copy for the
//! whole server, kept in the registry and edited in Settings › Review or with
//! the Scoring control's Save as default. Every view's scoring starts from
//! it, and every automatic stack build scores with it, so a refreshed stack
//! records the scoring the page compares it with.
//!
//! Before this copy existed, each person's browser or user preferences kept
//! the defaults. A page that finds the copy never stored saves the person's
//! own values once (see `useScoringDefaultsMigration` in the frontend).

use crate::db_registry::DbRegistry;
use crate::server::api::ApiResponse;
use crate::server::handlers::{require_registry_path, update_registry, AppError};
use crate::server::stack_preview::automatic::RefreshReason;
use crate::server::stack_preview::StackScoringSettings;
use crate::server::state::AppState;
use crate::user_preferences::{
    MAX_HFR_REJECT_LIMIT, MAX_SCORING_PENALTY, MAX_STAR_COUNT_REJECT_LIMIT, SCORING_PENALTY_STEP,
};
use axum::{extract::State, Json};
use serde::Serialize;
use std::sync::{Arc, PoisonError, RwLock};

static CURRENT: RwLock<Option<StackScoringSettings>> = RwLock::new(None);

/// Use the registry's scoring defaults, or the built-in ones when it stores
/// none.
pub fn configure(stored: Option<&StackScoringSettings>) {
    *CURRENT.write().unwrap_or_else(PoisonError::into_inner) = stored.copied();
}

/// The scoring automatic builds use now.
pub fn current() -> StackScoringSettings {
    CURRENT
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .unwrap_or_default()
}

/// Refuse values the Scoring control cannot set: a penalty outside 0–200%
/// or off its 10% steps, a Max HFR outside (0, 100] pixels, or a Min stars
/// that is not a whole number from 1 to 1,000,000. A penalty within float
/// noise of a step is put on it, so 0.30000000000000004 stores as 0.3.
fn checked(scoring: StackScoringSettings) -> Result<StackScoringSettings, String> {
    let penalty = |label: &str, value: f64| {
        let steps = value / SCORING_PENALTY_STEP;
        if !value.is_finite()
            || !(0.0..=MAX_SCORING_PENALTY).contains(&value)
            || (steps - steps.round()).abs() > 1e-6
        {
            return Err(format!(
                "the {label} penalty must be from 0% to {}% in steps of {}%",
                MAX_SCORING_PENALTY * 100.0,
                SCORING_PENALTY_STEP * 100.0
            ));
        }
        // Dividing by the steps per whole keeps a tenth exact.
        Ok(steps.round() / (1.0 / SCORING_PENALTY_STEP).round())
    };
    let hfr_reject_above = match scoring.hfr_reject_above {
        Some(limit) if !(limit.is_finite() && limit > 0.0 && limit <= MAX_HFR_REJECT_LIMIT) => {
            return Err(format!(
                "Max HFR must be above 0 and at most {MAX_HFR_REJECT_LIMIT} pixels, or off"
            ));
        }
        limit => limit,
    };
    let star_count_reject_below = match scoring.star_count_reject_below {
        Some(limit)
            if !(limit.fract() == 0.0
                && limit >= 1.0
                && limit <= MAX_STAR_COUNT_REJECT_LIMIT as f64) =>
        {
            return Err(format!(
                "Min stars must be a whole number from 1 to {MAX_STAR_COUNT_REJECT_LIMIT}, or off"
            ));
        }
        limit => limit,
    };
    Ok(StackScoringSettings {
        penalty_satellite: penalty("satellite trail", scoring.penalty_satellite)?,
        penalty_pointing: penalty("pointing", scoring.penalty_pointing)?,
        penalty_temporal: penalty("temporal", scoring.penalty_temporal)?,
        penalty_hfr_rise: penalty("HFR rise", scoring.penalty_hfr_rise)?,
        penalty_star_loss: penalty("star loss", scoring.penalty_star_loss)?,
        hfr_reject_above,
        star_count_reject_below,
    })
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScoringDefaultsResponse {
    /// What every view starts from and automatic builds use now.
    pub scoring: StackScoringSettings,
    /// PSF Guard's own values, which Reset goes back to.
    pub builtin: StackScoringSettings,
    /// Whether the server holds a copy. Until it does, `scoring` is the
    /// built-in values and a page may save the person's earlier defaults.
    pub stored: bool,
}

fn response(scoring: Option<StackScoringSettings>, stored: bool) -> ScoringDefaultsResponse {
    ScoringDefaultsResponse {
        scoring: scoring.unwrap_or_default(),
        builtin: StackScoringSettings::default(),
        stored,
    }
}

/// GET /api/settings/scoring
pub async fn get_scoring_defaults(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<ScoringDefaultsResponse>>, AppError> {
    // A server without a persistent registry has nothing to save to, so a
    // page there never tries to copy its own values over.
    let body = match require_registry_path(&state) {
        Ok(path) => {
            let stored = DbRegistry::load_or_init(&path)
                .map_err(|error| AppError::InternalError(error.to_string()))?
                .scoring;
            response(stored, stored.is_some())
        }
        Err(_) => response(Some(current()), true),
    };
    Ok(Json(ApiResponse::success(body)))
}

/// PUT /api/settings/scoring
///
/// Applies to every view from its next load and to automatic builds from
/// now on. When the values change and automatic previews are on, every
/// database's stacked projects refresh, so their cards are built with the
/// new scoring.
pub async fn update_scoring_defaults(
    State(state): State<Arc<AppState>>,
    Json(request): Json<StackScoringSettings>,
) -> Result<Json<ApiResponse<ScoringDefaultsResponse>>, AppError> {
    let scoring = checked(request).map_err(AppError::BadRequest)?;
    let previous = update_registry(&state, |registry| {
        let previous = registry.scoring.unwrap_or_default();
        registry.scoring = Some(scoring);
        Ok(previous)
    })
    .await?;
    configure(Some(&scoring));
    if previous != scoring {
        for ctx in state.all_databases() {
            state
                .auto_stacks
                .touch_database(&ctx.id, RefreshReason::Scoring);
        }
    }
    Ok(Json(ApiResponse::success(response(Some(scoring), true))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_built_in_values_pass_and_a_penalty_is_put_on_its_step() {
        let builtin = StackScoringSettings::default();
        assert_eq!(checked(builtin), Ok(builtin));
        let noisy = StackScoringSettings {
            penalty_satellite: 0.1 + 0.2,
            hfr_reject_above: Some(4.5),
            star_count_reject_below: Some(50.0),
            ..builtin
        };
        let stored = checked(noisy).unwrap();
        assert_eq!(stored.penalty_satellite, 0.3);
        assert_eq!(stored.hfr_reject_above, Some(4.5));
        assert_eq!(stored.star_count_reject_below, Some(50.0));
    }

    #[test]
    fn values_the_scoring_control_cannot_set_are_refused() {
        let builtin = StackScoringSettings::default();
        for refused in [
            StackScoringSettings {
                penalty_pointing: 2.5,
                ..builtin
            },
            StackScoringSettings {
                penalty_temporal: 0.25,
                ..builtin
            },
            StackScoringSettings {
                penalty_star_loss: f64::NAN,
                ..builtin
            },
            StackScoringSettings {
                hfr_reject_above: Some(0.0),
                ..builtin
            },
            StackScoringSettings {
                hfr_reject_above: Some(MAX_HFR_REJECT_LIMIT + 1.0),
                ..builtin
            },
            StackScoringSettings {
                star_count_reject_below: Some(12.5),
                ..builtin
            },
            StackScoringSettings {
                star_count_reject_below: Some(0.0),
                ..builtin
            },
        ] {
            assert!(checked(refused).is_err(), "{refused:?}");
        }
    }

    #[test]
    fn a_registry_without_a_copy_reads_as_the_built_in_values_not_stored() {
        let body = response(None, false);
        assert_eq!(body.scoring, StackScoringSettings::default());
        assert!(!body.stored);
    }
}
