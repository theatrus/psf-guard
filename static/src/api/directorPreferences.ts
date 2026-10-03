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
export interface ObservingSettings {
  scope: PreferenceScope;
  scope_id: string;
  revision: number;
  overrides: ObservingOverrides;
  enabled: boolean | null;
  site_id: string | null;
}
export interface PreferenceSource { scope: PreferenceScope; id: string; revision: number }
export interface EffectiveObserving {
  enabled: boolean;
  resolved: { policy: ObservingPolicy; provenance: {
    weights: Record<PreferenceFactor, PreferenceSource>;
    importance: PreferenceSource;
    minimum_dwell_ms: PreferenceSource;
    switch_margin: PreferenceSource;
  } };
  settings: ObservingSettings[];
}
export interface ObservingDefaults {
  global_id: string;
  presets: Record<string, ObservingPolicy>;
  sites: { id: string; name: string; revision: number }[];
}
