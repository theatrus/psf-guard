//! Process-wide stack preview settings, persisted in the registry and applied
//! without a restart: the automatic refresh policy, and the method every
//! stack integrates its frames with.

use crate::db_registry::{DbRegistry, StackAutomationSettings};
use crate::server::api::ApiResponse;
use crate::server::handlers::{require_registry_path, update_registry, AppError};
use crate::server::stack_preview::automatic::{
    self, AutomationPolicy, MAX_DELAY_MINUTES, MAX_NEW_CHANNEL_WINDOW_DAYS,
};
use crate::server::stack_preview::{method, StackMethod};
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
    /// Automatic refreshes also stack channels with no stack yet, once one
    /// of their frames is recent.
    pub build_new_channels: bool,
    /// Days back a new channel's frames may reach; 0 takes any age.
    pub new_channel_window_days: u32,
    pub default_new_channel_window_days: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateStackSettingsRequest {
    pub automatic_previews: bool,
    /// Omitted keeps the current delay.
    #[serde(default)]
    pub arrival_delay_minutes: Option<u32>,
    #[serde(default)]
    pub grade_delay_minutes: Option<u32>,
    /// Omitted keeps the current choice.
    #[serde(default)]
    pub build_new_channels: Option<bool>,
    /// Omitted keeps the current window; 0 takes any age.
    #[serde(default)]
    pub new_channel_window_days: Option<u32>,
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
        build_new_channels: policy.build_new_channels,
        new_channel_window_days: policy.new_channel_window_days,
        default_new_channel_window_days: automatic::DEFAULT_NEW_CHANNEL_WINDOW_DAYS,
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
        build_new_channels: request
            .build_new_channels
            .unwrap_or(current.build_new_channels),
        new_channel_window_days: request
            .new_channel_window_days
            .unwrap_or(current.new_channel_window_days),
    };
    if policy.new_channel_window_days > MAX_NEW_CHANNEL_WINDOW_DAYS {
        return Err(AppError::BadRequest(format!(
            "the new-channel window must be at most {MAX_NEW_CHANNEL_WINDOW_DAYS} days, or 0 for any age"
        )));
    }
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

/// The registry entry for a policy and a method: nothing at all when both
/// are the defaults, so a registry that only ever held defaults stays clean.
fn stored(policy: AutomationPolicy, method: StackMethod) -> Option<StackAutomationSettings> {
    let defaults = AutomationPolicy::default();
    (policy != defaults || method != StackMethod::default()).then_some(StackAutomationSettings {
        automatic_previews: policy.enabled.then_some(true),
        arrival_delay_minutes: (policy.arrival_delay_minutes != defaults.arrival_delay_minutes)
            .then_some(policy.arrival_delay_minutes),
        grade_delay_minutes: (policy.grade_delay_minutes != defaults.grade_delay_minutes)
            .then_some(policy.grade_delay_minutes),
        method: (method != StackMethod::default()).then_some(method),
        build_new_channels: policy.build_new_channels.then_some(true),
        new_channel_window_days: (policy.new_channel_window_days
            != defaults.new_channel_window_days)
            .then_some(policy.new_channel_window_days),
    })
}

fn stored_method(settings: Option<&StackAutomationSettings>) -> StackMethod {
    settings
        .and_then(|settings| settings.method)
        .unwrap_or_default()
}

/// PUT /api/settings/stacking
pub async fn update_stack_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateStackSettingsRequest>,
) -> Result<Json<ApiResponse<StackSettingsResponse>>, AppError> {
    let policy = update_registry(&state, |registry| {
        let policy = requested_policy(&request, registry.stacking.as_ref())?;
        registry.stacking = stored(policy, stored_method(registry.stacking.as_ref()));
        Ok(policy)
    })
    .await?;
    automatic::configure(policy);
    if !policy.enabled {
        state.auto_stacks.clear();
    }
    Ok(Json(ApiResponse::success(response(policy))))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StackMethodResponse {
    /// What new stacks use now.
    pub method: StackMethod,
    /// Seiza's recommended method, the default.
    pub recommended: StackMethod,
    /// What PSF Guard did before the method could be chosen.
    pub classic: StackMethod,
}

fn method_response(method: StackMethod) -> StackMethodResponse {
    StackMethodResponse {
        method,
        recommended: StackMethod::default(),
        classic: StackMethod::classic(),
    }
}

/// GET /api/settings/stacking/method
pub async fn get_stack_method(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<StackMethodResponse>>, AppError> {
    let method = match require_registry_path(&state) {
        Ok(path) => stored_method(
            DbRegistry::load_or_init(&path)
                .map_err(|error| AppError::InternalError(error.to_string()))?
                .stacking
                .as_ref(),
        ),
        Err(_) => method::current(),
    };
    Ok(Json(ApiResponse::success(method_response(method))))
}

/// PUT /api/settings/stacking/method
///
/// Applies to stacks started from now on, including automatic refreshes.
/// Stacks already built keep the method they record, and show as out of date
/// beside the new one.
pub async fn update_stack_method(
    State(state): State<Arc<AppState>>,
    Json(method): Json<StackMethod>,
) -> Result<Json<ApiResponse<StackMethodResponse>>, AppError> {
    update_registry(&state, |registry| {
        let policy = registry
            .stacking
            .as_ref()
            .map(|settings| settings.policy())
            .unwrap_or_default();
        registry.stacking = stored(policy, method);
        Ok(())
    })
    .await?;
    method::configure(method);
    Ok(Json(ApiResponse::success(method_response(method))))
}

/// How much of the processor background work may take.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkerSettingsResponse {
    /// In effect now, 0.05–1.
    pub interactive_ratio: f64,
    pub background_ratio: f64,
    /// The server config file's values, which "default" goes back to.
    pub default_interactive_ratio: f64,
    pub default_background_ratio: f64,
    pub logical_cores: usize,
}

fn worker_response(state: &AppState) -> WorkerSettingsResponse {
    let policy = state.worker_policy();
    let base = state.configured_worker_policy();
    WorkerSettingsResponse {
        interactive_ratio: policy.interactive_ratio,
        background_ratio: policy.background_ratio,
        default_interactive_ratio: base.interactive_ratio,
        default_background_ratio: base.background_ratio,
        logical_cores: crate::concurrency::logical_cores(),
    }
}

/// GET /api/settings/workers
pub async fn get_worker_settings(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<WorkerSettingsResponse>> {
    Json(ApiResponse::success(worker_response(&state)))
}

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateWorkerSettingsRequest {
    /// `null` goes back to the config file's share.
    pub interactive_ratio: Option<f64>,
    pub background_ratio: Option<f64>,
}

/// The registry entry for a request: a share equal to the config file's is
/// not written, so changing the file later still reaches it.
fn stored_workers(
    request: &UpdateWorkerSettingsRequest,
    base: crate::concurrency::WorkerPolicy,
) -> Result<Option<crate::db_registry::WorkerSettings>, AppError> {
    let check = |label: &str, ratio: Option<f64>, default: f64| match ratio {
        Some(ratio) if !(0.05..=1.0).contains(&ratio) || !ratio.is_finite() => Err(
            AppError::BadRequest(format!("the {label} share must be between 5% and 100%")),
        ),
        Some(ratio) if (ratio - default).abs() < 1e-9 => Ok(None),
        other => Ok(other),
    };
    let settings = crate::db_registry::WorkerSettings {
        interactive_ratio: check(
            "interactive",
            request.interactive_ratio,
            base.interactive_ratio,
        )?,
        background_ratio: check(
            "background",
            request.background_ratio,
            base.background_ratio,
        )?,
    };
    Ok((settings != crate::db_registry::WorkerSettings::default()).then_some(settings))
}

/// PUT /api/settings/workers — applies to every pool sized from now on;
/// work already running keeps the threads it has.
pub async fn update_worker_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateWorkerSettingsRequest>,
) -> Result<Json<ApiResponse<WorkerSettingsResponse>>, AppError> {
    let workers = stored_workers(&request, state.configured_worker_policy())?;
    update_registry(&state, |registry| {
        registry.workers = workers;
        Ok(())
    })
    .await?;
    state.apply_worker_settings(workers.as_ref());
    Ok(Json(ApiResponse::success(worker_response(&state))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_policy_is_not_written_and_a_chosen_one_round_trips() {
        assert_eq!(
            stored(AutomationPolicy::default(), StackMethod::default()),
            None
        );
        let chosen = AutomationPolicy {
            enabled: true,
            arrival_delay_minutes: 5,
            grade_delay_minutes: 30,
            build_new_channels: false,
            new_channel_window_days: 7,
        };
        let entry = stored(chosen, StackMethod::default()).expect("a chosen policy is stored");
        assert_eq!(entry.method, None, "the recommended method is not written");
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
            method: None,
            build_new_channels: None,
            new_channel_window_days: None,
        };
        let policy = requested_policy(
            &UpdateStackSettingsRequest {
                automatic_previews: false,
                arrival_delay_minutes: None,
                grade_delay_minutes: Some(45),
                build_new_channels: None,
                new_channel_window_days: None,
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
                build_new_channels: None,
                new_channel_window_days: None,
            },
            None,
        )
        .is_err());
        assert!(requested_policy(
            &UpdateStackSettingsRequest {
                automatic_previews: true,
                arrival_delay_minutes: None,
                grade_delay_minutes: Some(MAX_DELAY_MINUTES + 1),
                build_new_channels: None,
                new_channel_window_days: None,
            },
            None,
        )
        .is_err());
    }

    #[test]
    fn a_chosen_method_is_stored_alone_and_survives_a_policy_change() {
        let draft = StackMethod {
            final_pass: crate::server::stack_preview::StackFinalPass::Draft,
            ..StackMethod::default()
        };
        let entry = stored(AutomationPolicy::default(), draft).expect("a chosen method is stored");
        assert_eq!(entry.automatic_previews, None);
        assert_eq!(entry.method, Some(draft));
        // The automation route rebuilds the entry; the method rides along.
        let policy = AutomationPolicy {
            enabled: true,
            ..AutomationPolicy::default()
        };
        let rebuilt = stored(policy, stored_method(Some(&entry))).unwrap();
        assert_eq!(rebuilt.method, Some(draft));
        assert_eq!(stored_method(None), StackMethod::default());
    }

    #[test]
    fn a_share_equal_to_the_config_file_is_not_stored_and_nonsense_is_refused() {
        let base = crate::concurrency::WorkerPolicy::default();
        let request = |interactive, background| UpdateWorkerSettingsRequest {
            interactive_ratio: interactive,
            background_ratio: background,
        };
        assert_eq!(stored_workers(&request(None, None), base).unwrap(), None);
        assert_eq!(
            stored_workers(&request(Some(base.interactive_ratio), None), base).unwrap(),
            None
        );
        let chosen = stored_workers(&request(Some(0.75), Some(0.1)), base)
            .unwrap()
            .unwrap();
        assert_eq!(chosen.interactive_ratio, Some(0.75));
        assert_eq!(chosen.background_ratio, Some(0.1));
        let applied = chosen.apply(base);
        assert_eq!(applied.interactive_ratio, 0.75);
        assert_eq!(applied.background_ratio, 0.1);
        assert!(stored_workers(&request(Some(0.0), None), base).is_err());
        assert!(stored_workers(&request(None, Some(1.5)), base).is_err());
    }
}
