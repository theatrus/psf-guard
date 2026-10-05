import type { PreferenceSource, SchedulingOverrides, SchedulingValues } from '../../api/directorPreferences';

export type LimitKey = keyof SchedulingValues;

/** One Target Scheduler scheduling limit, as the plan shows and edits it. */
export interface LimitSpec {
  key: LimitKey;
  label: string;
  kind: 'number' | 'flag';
  unit?: string;
  min?: number;
  max?: number;
  step?: number;
  /** What 0 means, when it means "off" rather than a value. */
  zero?: string;
  hint: string;
}

/** In the order Target Scheduler's own project editor lists them. */
export const LIMITS: LimitSpec[] = [
  { key: 'minimum_altitude_degrees', label: 'Minimum altitude', kind: 'number', unit: '°', min: 0, max: 90, step: 0.1, hint: 'Lowest altitude to image at.' },
  { key: 'maximum_altitude_degrees', label: 'Maximum altitude', kind: 'number', unit: '°', min: 0, max: 90, step: 0.1, zero: 'none', hint: 'Highest altitude to image at; 0 for no limit.' },
  { key: 'use_custom_horizon', label: 'Custom horizon', kind: 'flag', hint: "Follow the N.I.N.A. profile's custom horizon instead of a flat minimum altitude." },
  { key: 'horizon_offset_degrees', label: 'Horizon offset', kind: 'number', unit: '°', min: -90, max: 90, step: 0.1, hint: 'Degrees added to the custom horizon.' },
  { key: 'meridian_window_minutes', label: 'Meridian window', kind: 'number', unit: 'min', min: 0, max: 719, step: 1, zero: 'off', hint: 'Minutes either side of the meridian to avoid; 0 turns it off.' },
  { key: 'minimum_time_minutes', label: 'Minimum time', kind: 'number', unit: 'min', min: 0, max: 1440, step: 1, hint: 'Shortest stretch worth starting the project for.' },
  { key: 'dither_every', label: 'Dither every', kind: 'number', unit: 'frames', min: 0, max: 10000, step: 1, zero: 'per template', hint: "Exposures between dithers; 0 follows each exposure template's setting." },
  { key: 'filter_switch_frequency', label: 'Filter switch every', kind: 'number', unit: 'frames', min: 0, max: 10000, step: 1, zero: 'automatic', hint: 'Exposures before changing filter; 0 lets Target Scheduler decide.' },
  { key: 'smart_exposure_order', label: 'Smart exposure order', kind: 'flag', hint: 'Let Target Scheduler order exposures for the best conditions.' },
];

/** Target Scheduler's defaults, what an unset limit falls back to. */
export const TS_DEFAULTS: SchedulingValues = {
  minimum_time_minutes: 30, minimum_altitude_degrees: 0, maximum_altitude_degrees: 0, use_custom_horizon: false,
  horizon_offset_degrees: 0, meridian_window_minutes: 0, filter_switch_frequency: 0, dither_every: 0, smart_exposure_order: false,
};

export function describeLimit(spec: LimitSpec, value: number | boolean): string {
  if (spec.kind === 'flag') return value ? 'on' : 'off';
  if (value === 0 && spec.zero) return spec.zero;
  return `${value}${spec.unit === '°' ? '°' : ` ${spec.unit}`}`;
}

const SCOPE_NAMES: Record<string, string> = { global: 'every plan', site: 'site', rig: 'rig', project: 'this plan' };

/** Where a resolved value came from, in words. */
export function describeSource(source: PreferenceSource | undefined): string {
  return source ? `from ${SCOPE_NAMES[source.scope] ?? source.scope}` : 'Target Scheduler default';
}

/** Overrides with unset limits dropped, as the server stores them. */
export function compactOverrides(overrides: SchedulingOverrides): SchedulingOverrides {
  return Object.fromEntries(Object.entries(overrides).filter(([, value]) => value !== null && value !== undefined)) as SchedulingOverrides;
}

/** What a scope inherits from the scopes above it, most general first: each
 *  value and where it comes from. Unset everywhere is Target Scheduler's. */
export function inheritedLimits(layers: Array<{ overrides?: SchedulingOverrides; from: string } | null>): { values: SchedulingValues; from: Record<LimitKey, string> } {
  const values = { ...TS_DEFAULTS };
  const from = Object.fromEntries(LIMITS.map(spec => [spec.key, 'Target Scheduler default'])) as Record<LimitKey, string>;
  for (const layer of layers) {
    if (!layer?.overrides) continue;
    for (const spec of LIMITS) {
      const value = layer.overrides[spec.key];
      if (value === null || value === undefined) continue;
      (values as Record<LimitKey, number | boolean>)[spec.key] = value;
      from[spec.key] = layer.from;
    }
  }
  return { values, from };
}
