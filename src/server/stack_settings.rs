//! Process-wide automatic stack preview settings, persisted in the registry
//! and applied to the scheduler without a restart.

use crate::db_registry::{DbRegistry, StackAutomationSettings};
use crate::server::api::ApiResponse;
use crate::server::handlers::{require_registry_path, AppError};
use crate::server::stack_preview::automatic::{self, AutomationPolicy, MAX_DELAY_MINUTES};
use crate::server::state::AppState;
use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackSettingsResponse {
    /// Remembered stack previews rebuild on their own when frames arrive, a
    /// sync lands, or grades change.
    pub automatic_previews: bool,
    /// Minutes an arrival or sync settles before the refresh runs.
    pub arrival_delay_minutes: u32,
    /// Minutes a grade change settles before the refresh runs.
    pub grade_delay_minutes: u32,
    pub default_arrival_delay_minutes: u32,
    pub default_grade_delay_minutes: u32,
    pub max_delay_minutes: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateStackSettingsRequest {
    pub automatic_previews: bool,
    /// Omitted keeps the current delay.
    #[serde(default)]
    pub arrival_delay_minutes: Option<u32>,
    #[serde(default)]
    pub grade_delay_minutes: Option<u32>,
}

fn response(policy: AutomationPolicy) -> StackSettingsResponse {
    let defaults = AutomationPolicy::default();
    StackSettingsResponse {
        automatic_previews: policy.enabled,
        arrival_delay_minutes: policy.arrival_delay_minutes,
        grade_delay_minutes: policy.grade_delay_minutes,
        default_arrival_delay_minutes: defaults.arrival_delay_minutes,
        default_grade_delay_minutes: defaults.grade_delay_minutes,
        max_delay_minutes: MAX_DELAY_MINUTES,
    }
}

/// GET /api/settings/stacking
pub async fn get_stack_settings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<StackSettingsResponse>>, AppError> {
    // A server without a persistent registry still has a current value: what
    // the scheduler runs with now.
    let policy = match require_registry_path(&state) {
        Ok(path) => DbRegistry::load_or_init(&path)
            .map_err(|error| AppError::InternalError(error.to_string()))?
            .stacking
            .map(|settings| settings.policy())
            .unwrap_or_default(),
        Err(_) => automatic::policy(),
    };
    Ok(Json(ApiResponse::success(response(policy))))
}

/// What one update asks the policy to become, over what is stored now.
fn requested_policy(
    request: &UpdateStackSettingsRequest,
    current: Option<&StackAutomationSettings>,
) -> Result<AutomationPolicy, AppError> {
    let current = current
        .map(|settings| settings.policy())
        .unwrap_or_default();
    let policy = AutomationPolicy {
        enabled: request.automatic_previews,
        arrival_delay_minutes: request
            .arrival_delay_minutes
            .unwrap_or(current.arrival_delay_minutes),
        grade_delay_minutes: request
            .grade_delay_minutes
            .unwrap_or(current.grade_delay_minutes),
    };
    for (label, minutes) in [
        ("arrival", policy.arrival_delay_minutes),
        ("grade", policy.grade_delay_minutes),
    ] {
        if !(1..=MAX_DELAY_MINUTES).contains(&minutes) {
            return Err(AppError::BadRequest(format!(
                "the {label} delay must be between 1 and {MAX_DELAY_MINUTES} minutes"
            )));
        }
    }
    Ok(policy)
}

/// The registry entry for a policy: nothing at all when it is the default,
/// so a registry that only ever held defaults stays clean.
fn stored(policy: AutomationPolicy) -> Option<StackAutomationSettings> {
    let defaults = AutomationPolicy::default();
    (policy != defaults).then_some(StackAutomationSettings {
        automatic_previews: policy.enabled.then_some(true),
        arrival_delay_minutes: (policy.arrival_delay_minutes != defaults.arrival_delay_minutes)
            .then_some(policy.arrival_delay_minutes),
        grade_delay_minutes: (policy.grade_delay_minutes != defaults.grade_delay_minutes)
            .then_some(policy.grade_delay_minutes),
    })
}

/// PUT /api/settings/stacking
pub async fn update_stack_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateStackSettingsRequest>,
) -> Result<Json<ApiResponse<StackSettingsResponse>>, AppError> {
    let path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut registry = DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let policy = requested_policy(&request, registry.stacking.as_ref())?;
    registry.stacking = stored(policy);
    registry
        .save(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    automatic::configure(policy);
    if !policy.enabled {
        state.auto_stacks.clear();
    }
    Ok(Json(ApiResponse::success(response(policy))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_policy_is_not_written_and_a_chosen_one_round_trips() {
        assert_eq!(stored(AutomationPolicy::default()), None);
        let chosen = AutomationPolicy {
            enabled: true,
            arrival_delay_minutes: 5,
            grade_delay_minutes: 30,
        };
        let entry = stored(chosen).expect("a chosen policy is stored");
        assert_eq!(entry.automatic_previews, Some(true));
        assert_eq!(
            entry.arrival_delay_minutes, None,
            "the default delay is not written"
        );
        assert_eq!(entry.grade_delay_minutes, Some(30));
        assert_eq!(entry.policy(), chosen);
    }

    #[test]
    fn an_update_keeps_the_delays_it_omits_and_refuses_nonsense() {
        let current = StackAutomationSettings {
            automatic_previews: Some(true),
            arrival_delay_minutes: Some(10),
            grade_delay_minutes: None,
        };
        let policy = requested_policy(
            &UpdateStackSettingsRequest {
                automatic_previews: false,
                arrival_delay_minutes: None,
                grade_delay_minutes: Some(45),
            },
            Some(&current),
        )
        .unwrap();
        assert!(!policy.enabled);
        assert_eq!(policy.arrival_delay_minutes, 10);
        assert_eq!(policy.grade_delay_minutes, 45);
        assert!(requested_policy(
            &UpdateStackSettingsRequest {
                automatic_previews: true,
                arrival_delay_minutes: Some(0),
                grade_delay_minutes: None,
            },
            None,
        )
        .is_err());
        assert!(requested_policy(
            &UpdateStackSettingsRequest {
                automatic_previews: true,
                arrival_delay_minutes: None,
                grade_delay_minutes: Some(MAX_DELAY_MINUTES + 1),
            },
            None,
        )
        .is_err());
    }
}
