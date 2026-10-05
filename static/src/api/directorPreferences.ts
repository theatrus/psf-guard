export type PreferenceScope = 'global' | 'site' | 'rig' | 'project';
export type PreferenceFactor = 'importance' | 'window_urgency' | 'altitude' | 'moon_opportunity' | 'completion' | 'efficiency' | 'continuity';
export interface ObservingPolicy {
  weights: Record<PreferenceFactor, number>;
  importance: number;
  minimum_dwell_ms: number;
  switch_margin: number;
}
export interface ObservingOverrides {
  weights: Partial<Record<PreferenceFactor, number>>;
  importance: number | null;
  minimum_dwell_ms: number | null;
  switch_margin: number | null;
}
/** Target Scheduler's per-project scheduling limits; each `null`/absent
 *  value inherits from the scope above, then Target Scheduler's default. */
export interface SchedulingOverrides {
  minimum_time_minutes?: number | null;
  minimum_altitude_degrees?: number | null;
  maximum_altitude_degrees?: number | null;
  use_custom_horizon?: boolean | null;
  horizon_offset_degrees?: number | null;
  meridian_window_minutes?: number | null;
  filter_switch_frequency?: number | null;
  dither_every?: number | null;
  smart_exposure_order?: boolean | null;
}
export interface SchedulingValues {
  minimum_time_minutes: number;
  minimum_altitude_degrees: number;
  maximum_altitude_degrees: number;
  use_custom_horizon: boolean;
  horizon_offset_degrees: number;
  meridian_window_minutes: number;
  filter_switch_frequency: number;
  dither_every: number;
  smart_exposure_order: boolean;
}
export interface ObservingSettings {
  scope: PreferenceScope;
  scope_id: string;
  revision: number;
  overrides: ObservingOverrides;
  enabled: boolean | null;
  site_id: string | null;
  project_order?: string[] | null;
  scheduling?: SchedulingOverrides;
}
export interface PreferenceSource { scope: PreferenceScope; id: string; revision: number }
export interface EffectiveObserving {
  project_order: string[] | null;
  order_source: PreferenceSource | null;
  enabled: boolean;
  resolved: { policy: ObservingPolicy; provenance: {
    weights: Record<PreferenceFactor, PreferenceSource>;
    importance: PreferenceSource;
    minimum_dwell_ms: PreferenceSource;
    switch_margin: PreferenceSource;
  } };
  settings: ObservingSettings[];
  /** Resolved limits for the rig and project, and the scope behind each. */
  scheduling?: { values: SchedulingValues; sources: Partial<Record<keyof SchedulingValues, PreferenceSource>> };
}
export interface ObservingDefaults {
  global_id: string;
  presets: Record<string, ObservingPolicy>;
  sites: { id: string; name: string; revision: number }[];
}
